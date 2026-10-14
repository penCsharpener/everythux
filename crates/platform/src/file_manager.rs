//! File manager integration (ROADMAP Phase 5).
//!
//! Fallback chain: `org.freedesktop.FileManager1` on the session bus → the
//! `org.freedesktop.portal.OpenURI` portal → `xdg-open`.
//!
//! Every function here is **blocking** (D-Bus round trips, process spawns). Call them from a worker
//! thread, never from the UI thread. Helper programs are started detached and reaped on a small
//! background thread, so the caller never waits for them and no zombies are left behind.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::blocking::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::zvariant::{Fd, Value};

use crate::PlatformError;
use crate::uri::file_uri;

const FM_NAME: &str = "org.freedesktop.FileManager1";
const FM_PATH: &str = "/org/freedesktop/FileManager1";
const FM_IFACE: &str = "org.freedesktop.FileManager1";

const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_IFACE: &str = "org.freedesktop.portal.OpenURI";

/// A file manager may have to be started first, so give calls some time before giving up.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// How a request was fulfilled (for logging / status bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `org.freedesktop.FileManager1` on the session bus.
    FileManager1,
    /// `org.freedesktop.portal.OpenURI`.
    Portal,
    /// `xdg-open` (no selection / properties possible).
    XdgOpen,
    /// A program started from an "Open with" or service-menu entry.
    Launched,
    /// Files put on the clipboard through a helper program.
    Clipboard,
}

/// Opens the folder containing `path` with `path` selected (Path column double-click).
///
/// Tries `FileManager1.ShowItems`, then the `OpenURI.OpenDirectory` portal, then `xdg-open` on the
/// parent directory (which cannot select the file). Blocking: call from a worker thread.
pub fn show_item(path: &Path) -> Result<Method, PlatformError> {
    show_item_with(&Services::default(), path, &startup_id())
}

/// Opens the file manager's properties dialog for `path` (Size/Modified double-click).
/// Returns `Err(PlatformError::Unsupported)` if no file manager implements
/// `ShowItemProperties`; the UI then shows its built-in dialog.
///
/// Blocking: call from a worker thread.
pub fn show_properties(path: &Path) -> Result<Method, PlatformError> {
    show_properties_with(&Services::default(), path, &startup_id())
}

/// Opens `path` with its default application (Name column double-click / Enter).
///
/// Uses the `OpenURI.OpenFile` portal when available, otherwise `xdg-open`. Blocking: call from a
/// worker thread.
pub fn open_default(path: &Path) -> Result<Method, PlatformError> {
    open_default_with(&Services::default(), path, &startup_id())
}

/// The bus names and helper program used by the implementation; tests substitute their own.
#[derive(Debug, Clone, Copy)]
struct Services<'a> {
    file_manager: &'a str,
    portal: &'a str,
    xdg_open: &'a str,
}

impl Default for Services<'static> {
    fn default() -> Self {
        Self {
            file_manager: FM_NAME,
            portal: PORTAL_NAME,
            xdg_open: "xdg-open",
        }
    }
}

fn show_item_with(services: &Services<'_>, path: &Path, startup_id: &str) -> Result<Method, PlatformError> {
    match session_connection() {
        Ok(conn) => {
            if is_available(&conn, services.file_manager) {
                let uris = vec![file_uri(path)];
                match call_file_manager(&conn, services, "ShowItems", &(uris, startup_id)) {
                    Ok(()) => return Ok(Method::FileManager1),
                    Err(err) => tracing::warn!(%err, "FileManager1.ShowItems failed, trying portal"),
                }
            }
            if is_available(&conn, services.portal) {
                match call_portal(&conn, services, "OpenDirectory", path, startup_id) {
                    Ok(()) => return Ok(Method::Portal),
                    Err(err) => tracing::warn!(%err, "portal OpenDirectory failed, using xdg-open"),
                }
            }
        }
        Err(err) => tracing::debug!(%err, "no session bus, using xdg-open"),
    }
    let target = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(path);
    xdg_open(services.xdg_open, target)
}

fn show_properties_with(
    services: &Services<'_>,
    path: &Path,
    startup_id: &str,
) -> Result<Method, PlatformError> {
    const NONE: PlatformError =
        PlatformError::Unsupported("no file manager implements FileManager1.ShowItemProperties");
    let conn = session_connection().map_err(|err| {
        tracing::debug!(%err, "no session bus for ShowItemProperties");
        NONE
    })?;
    if !is_available(&conn, services.file_manager) {
        return Err(NONE);
    }
    let uris = vec![file_uri(path)];
    match call_file_manager(&conn, services, "ShowItemProperties", &(uris, startup_id)) {
        Ok(()) => Ok(Method::FileManager1),
        Err(CallError::Missing) => Err(NONE),
        Err(CallError::Failed(err)) => Err(err),
    }
}

fn open_default_with(
    services: &Services<'_>,
    path: &Path,
    startup_id: &str,
) -> Result<Method, PlatformError> {
    // The portal's OpenFile is for files; directories go to the default file manager via xdg-open.
    if !path.is_dir() {
        match session_connection() {
            Ok(conn) if is_available(&conn, services.portal) => {
                match call_portal(&conn, services, "OpenFile", path, startup_id) {
                    Ok(()) => return Ok(Method::Portal),
                    Err(err) => tracing::warn!(%err, "portal OpenFile failed, using xdg-open"),
                }
            }
            Ok(_) => {}
            Err(err) => tracing::debug!(%err, "no session bus, using xdg-open"),
        }
    }
    xdg_open(services.xdg_open, path)
}

/// Outcome of a failed D-Bus call.
#[derive(Debug)]
enum CallError {
    /// The service, object, interface or method does not exist / is not supported.
    Missing,
    /// Any other failure.
    Failed(PlatformError),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("not supported by the service"),
            Self::Failed(err) => err.fmt(f),
        }
    }
}

impl From<zbus::Error> for CallError {
    fn from(err: zbus::Error) -> Self {
        if let zbus::Error::MethodError(name, ..) = &err
            && is_missing_error_name(name.as_str())
        {
            return Self::Missing;
        }
        if matches!(err, zbus::Error::InterfaceNotFound | zbus::Error::Unsupported) {
            return Self::Missing;
        }
        Self::Failed(PlatformError::dbus(err))
    }
}

/// D-Bus error names meaning "this peer does not offer that".
fn is_missing_error_name(name: &str) -> bool {
    name.strip_prefix("org.freedesktop.DBus.Error.")
        .is_some_and(|rest| {
            matches!(
                rest,
                "UnknownMethod"
                    | "UnknownObject"
                    | "UnknownInterface"
                    | "ServiceUnknown"
                    | "NameHasNoOwner"
                    | "NotSupported"
            ) || rest.starts_with("Spawn.")
        })
        || name == "org.freedesktop.portal.Error.NotSupported"
}

fn session_connection() -> zbus::Result<Connection> {
    Builder::session()?.method_timeout(CALL_TIMEOUT).build()
}

/// True if `name` currently has an owner or can be started by the bus.
fn is_available(conn: &Connection, name: &str) -> bool {
    let check = || -> zbus::Result<bool> {
        let bus_name = BusName::try_from(name)?;
        let proxy = DBusProxy::new(conn)?;
        if proxy.name_has_owner(bus_name)? {
            return Ok(true);
        }
        Ok(proxy.list_activatable_names()?.iter().any(|n| n.as_str() == name))
    };
    check().unwrap_or_else(|err| {
        tracing::debug!(%err, name, "cannot query bus for service");
        false
    })
}

fn call_file_manager(
    conn: &Connection,
    services: &Services<'_>,
    method: &str,
    body: &(Vec<String>, &str),
) -> Result<(), CallError> {
    conn.call_method(Some(services.file_manager), FM_PATH, Some(FM_IFACE), method, body)?;
    Ok(())
}

/// Calls `OpenURI.<method>(parent_window, fd, options)` with an fd for `path` opened read-only.
fn call_portal(
    conn: &Connection,
    services: &Services<'_>,
    method: &str,
    path: &Path,
    startup_id: &str,
) -> Result<(), CallError> {
    let file = File::open(path).map_err(|err| {
        CallError::Failed(PlatformError::dbus(format_args!(
            "cannot open {} for the portal: {err}",
            path.display()
        )))
    })?;
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    if !startup_id.is_empty() {
        options.insert("activation_token", Value::from(startup_id));
    }
    conn.call_method(
        Some(services.portal),
        PORTAL_PATH,
        Some(PORTAL_IFACE),
        method,
        &("", Fd::from(&file), options),
    )?;
    Ok(())
}

/// Startup notification / activation token to pass along, or `""`.
fn startup_id() -> String {
    startup_id_from(
        std::env::var_os("DESKTOP_STARTUP_ID"),
        std::env::var_os("XDG_ACTIVATION_TOKEN"),
    )
}

fn startup_id_from(desktop_startup_id: Option<OsString>, activation_token: Option<OsString>) -> String {
    desktop_startup_id
        .filter(|v| !v.is_empty())
        .or_else(|| activation_token.filter(|v| !v.is_empty()))
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn xdg_open(program: &str, target: &Path) -> Result<Method, PlatformError> {
    spawn_detached(OsStr::new(program), &safe_arg(target))?;
    Ok(Method::XdgOpen)
}

/// Makes sure a path cannot be mistaken for an option or a URL scheme by the helper.
fn safe_arg(path: &Path) -> Cow<'_, Path> {
    if path.is_absolute() {
        Cow::Borrowed(path)
    } else {
        Cow::Owned(Path::new(".").join(path))
    }
}

/// Starts `program arg` detached (null stdio, own process group) and reaps it on a background
/// thread. Never waits on the calling thread.
fn spawn_detached(program: &OsStr, arg: &Path) -> Result<(), PlatformError> {
    spawn_and_reap(program, arg).map(drop)
}

/// Like [`spawn_detached`], but returns the reaper thread so tests can join it.
fn spawn_and_reap(
    program: &OsStr,
    arg: &Path,
) -> Result<Option<JoinHandle<io::Result<ExitStatus>>>, PlatformError> {
    let mut child = Command::new(program)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|source| PlatformError::Spawn {
            program: program.to_string_lossy().into_owned(),
            source,
        })?;
    let reaper = std::thread::Builder::new()
        .name("helper-reaper".to_owned())
        .spawn(move || child.wait());
    match reaper {
        Ok(handle) => Ok(Some(handle)),
        Err(err) => {
            tracing::warn!(%err, "cannot start reaper thread; helper may stay a zombie until exit");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver, Sender};

    use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue};

    use super::*;

    const NEEDS_BUS: &str = "needs a D-Bus session bus (run with dbus-run-session)";

    // ---- pure helpers ----

    #[test]
    fn startup_id_prefers_desktop_startup_id() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(startup_id_from(os("a"), os("b")), "a");
        assert_eq!(startup_id_from(None, os("b")), "b");
        assert_eq!(startup_id_from(os(""), os("b")), "b");
        assert_eq!(startup_id_from(None, None), "");
    }

    #[test]
    fn missing_error_names() {
        for name in [
            "org.freedesktop.DBus.Error.UnknownMethod",
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NotSupported",
            "org.freedesktop.DBus.Error.Spawn.ServiceNotFound",
        ] {
            assert!(is_missing_error_name(name), "{name}");
        }
        for name in ["org.freedesktop.DBus.Error.NoReply", "org.example.Error.Boom"] {
            assert!(!is_missing_error_name(name), "{name}");
        }
    }

    #[test]
    fn relative_paths_cannot_look_like_options() {
        assert_eq!(safe_arg(Path::new("/a/b")).as_ref(), Path::new("/a/b"));
        assert_eq!(safe_arg(Path::new("-rf")).as_ref(), Path::new("./-rf"));
    }

    #[test]
    fn spawn_runs_detached_and_is_reaped() {
        let handle = spawn_and_reap(OsStr::new("true"), Path::new("/"))
            .expect("spawn true")
            .expect("reaper thread");
        let status = handle.join().expect("reaper did not panic").expect("wait");
        assert!(status.success());
    }

    #[test]
    fn spawn_failure_is_reported() {
        let err =
            spawn_detached(OsStr::new("everythux-no-such-program"), Path::new("/")).expect_err("must fail");
        assert!(
            matches!(err, PlatformError::Spawn { ref program, .. } if program.contains("no-such")),
            "{err:?}"
        );
    }

    // ---- mock services on the private session bus ----

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        ShowItems(Vec<String>, String),
        ShowItemProperties(Vec<String>, String),
        /// Method name, parent window, contents of the passed fd, option keys.
        Portal(&'static str, String, String, Vec<String>),
    }

    struct FullFm(Sender<Call>);

    #[zbus::interface(name = "org.freedesktop.FileManager1")]
    impl FullFm {
        fn show_items(&self, uris: Vec<String>, startup_id: String) {
            self.0
                .send(Call::ShowItems(uris, startup_id))
                .expect("test alive");
        }

        fn show_item_properties(&self, uris: Vec<String>, startup_id: String) {
            self.0
                .send(Call::ShowItemProperties(uris, startup_id))
                .expect("test alive");
        }
    }

    struct ItemsOnlyFm(Sender<Call>);

    #[zbus::interface(name = "org.freedesktop.FileManager1")]
    impl ItemsOnlyFm {
        fn show_items(&self, uris: Vec<String>, startup_id: String) {
            self.0
                .send(Call::ShowItems(uris, startup_id))
                .expect("test alive");
        }
    }

    struct MockPortal(Sender<Call>);

    impl MockPortal {
        fn record(
            &self,
            method: &'static str,
            parent: String,
            fd: OwnedFd,
            options: HashMap<String, OwnedValue>,
        ) -> OwnedObjectPath {
            let mut contents = String::new();
            let file = File::from(std::os::fd::OwnedFd::from(fd));
            io::Read::read_to_string(&mut &file, &mut contents).expect("read passed fd");
            let mut keys: Vec<String> = options.into_keys().collect();
            keys.sort();
            self.0
                .send(Call::Portal(method, parent, contents, keys))
                .expect("test alive");
            OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/mock").expect("valid path")
        }
    }

    #[zbus::interface(name = "org.freedesktop.portal.OpenURI")]
    impl MockPortal {
        fn open_directory(
            &self,
            parent_window: String,
            fd: OwnedFd,
            options: HashMap<String, OwnedValue>,
        ) -> OwnedObjectPath {
            self.record("OpenDirectory", parent_window, fd, options)
        }

        fn open_file(
            &self,
            parent_window: String,
            fd: OwnedFd,
            options: HashMap<String, OwnedValue>,
        ) -> OwnedObjectPath {
            self.record("OpenFile", parent_window, fd, options)
        }
    }

    /// Serves `iface` at `path` under `name` on the session bus; the connection must stay alive.
    fn serve<I: zbus::object_server::Interface>(name: &str, path: &str, iface: I) -> Connection {
        Builder::session()
            .and_then(|b| b.name(name.to_owned()))
            .and_then(|b| b.serve_at(path.to_owned(), iface))
            .and_then(Builder::build)
            .expect("mock service on session bus")
    }

    /// A bus name unique to this test run so parallel tests don't collide.
    fn unique(tag: &str) -> String {
        format!("org.everythux.test.{tag}.p{}", std::process::id())
    }

    fn channel() -> (Sender<Call>, Receiver<Call>) {
        mpsc::channel()
    }

    fn recv(rx: &Receiver<Call>) -> Call {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mock received a call")
    }

    fn non_utf8_path() -> PathBuf {
        PathBuf::from(OsStr::from_bytes(b"/tmp/dir with space/#odd?\xff.txt"))
    }

    const NON_UTF8_URI: &str = "file:///tmp/dir%20with%20space/%23odd%3F%FF.txt";

    #[test]
    #[ignore = "needs a D-Bus session bus (run with dbus-run-session)"]
    fn show_item_calls_show_items_with_encoded_uri() {
        let _ = NEEDS_BUS;
        let fm = unique("show_item");
        let (tx, rx) = channel();
        let _service = serve(&fm, FM_PATH, FullFm(tx));
        let services = Services {
            file_manager: &fm,
            portal: "org.everythux.test.Absent",
            xdg_open: "false",
        };

        let method = show_item_with(&services, &non_utf8_path(), "tok-1").expect("show_item");
        assert_eq!(method, Method::FileManager1);
        assert_eq!(
            recv(&rx),
            Call::ShowItems(vec![NON_UTF8_URI.to_owned()], "tok-1".to_owned())
        );

        let plain = Path::new("/home/me/My Docs/a#b.txt");
        show_item_with(&services, plain, "").expect("show_item");
        assert_eq!(
            recv(&rx),
            Call::ShowItems(
                vec!["file:///home/me/My%20Docs/a%23b.txt".to_owned()],
                String::new()
            )
        );
    }

    #[test]
    #[ignore = "needs a D-Bus session bus (run with dbus-run-session)"]
    fn public_show_item_and_properties_use_the_well_known_name() {
        // The only test that claims the real name, so parallel tests cannot collide with it.
        let (tx, rx) = channel();
        let _service = serve(FM_NAME, FM_PATH, FullFm(tx));
        let path = Path::new("/srv/x y");
        assert_eq!(show_item(path).expect("show_item"), Method::FileManager1);
        assert!(matches!(recv(&rx), Call::ShowItems(uris, _) if uris == ["file:///srv/x%20y"]));
        assert_eq!(
            show_properties(path).expect("show_properties"),
            Method::FileManager1
        );
        assert!(matches!(recv(&rx), Call::ShowItemProperties(uris, _) if uris == ["file:///srv/x%20y"]));
    }

    #[test]
    #[ignore = "needs a D-Bus session bus (run with dbus-run-session)"]
    fn show_properties_calls_show_item_properties() {
        let fm = unique("props");
        let (tx, rx) = channel();
        let _service = serve(&fm, FM_PATH, FullFm(tx));
        let services = Services {
            file_manager: &fm,
            portal: "org.everythux.test.Absent",
            xdg_open: "false",
        };

        let method = show_properties_with(&services, &non_utf8_path(), "tok-2").expect("props");
        assert_eq!(method, Method::FileManager1);
        assert_eq!(
            recv(&rx),
            Call::ShowItemProperties(vec![NON_UTF8_URI.to_owned()], "tok-2".to_owned())
        );
    }

    #[test]
    #[ignore = "needs a D-Bus session bus (run with dbus-run-session)"]
    fn show_properties_is_unsupported_when_method_is_missing() {
        let fm = unique("items_only");
        let (tx, _rx) = channel();
        let _service = serve(&fm, FM_PATH, ItemsOnlyFm(tx));
        let services = Services {
            file_manager: &fm,
            portal: "org.everythux.test.Absent",
            xdg_open: "false",
        };

        let err = show_properties_with(&services, Path::new("/tmp"), "").expect_err("unsupported");
        assert!(matches!(err, PlatformError::Unsupported(_)), "{err:?}");
    }

    #[test]
    #[ignore = "needs a D-Bus session bus (run with dbus-run-session)"]
    fn show_properties_is_unsupported_without_a_file_manager() {
        let services = Services {
            file_manager: "org.everythux.test.AbsentFm",
            portal: "org.everythux.test.Absent",
            xdg_open: "false",
        };
        let err = show_properties_with(&services, Path::new("/tmp"), "").expect_err("unsupported");
        assert!(matches!(err, PlatformError::Unsupported(_)), "{err:?}");
    }

    #[test]
    #[ignore = "needs a D-Bus session bus (run with dbus-run-session)"]
    fn show_item_falls_back_to_portal_with_an_fd() {
        let portal = unique("portal_dir");
        let (tx, rx) = channel();
        let _service = serve(&portal, PORTAL_PATH, MockPortal(tx));
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("hello world.txt");
        std::fs::write(&file, "payload").expect("write");
        let services = Services {
            file_manager: "org.everythux.test.AbsentFm",
            portal: &portal,
            xdg_open: "false",
        };

        let method = show_item_with(&services, &file, "act-tok").expect("show_item");
        assert_eq!(method, Method::Portal);
        assert_eq!(
            recv(&rx),
            Call::Portal(
                "OpenDirectory",
                String::new(),
                "payload".to_owned(),
                vec!["activation_token".to_owned()]
            )
        );
    }

    #[test]
    #[ignore = "needs a D-Bus session bus (run with dbus-run-session)"]
    fn open_default_uses_portal_open_file() {
        let portal = unique("portal_file");
        let (tx, rx) = channel();
        let _service = serve(&portal, PORTAL_PATH, MockPortal(tx));
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("doc.txt");
        std::fs::write(&file, "body").expect("write");
        let services = Services {
            file_manager: "org.everythux.test.AbsentFm",
            portal: &portal,
            xdg_open: "false",
        };

        assert_eq!(
            open_default_with(&services, &file, "").expect("open"),
            Method::Portal
        );
        assert_eq!(
            recv(&rx),
            Call::Portal("OpenFile", String::new(), "body".to_owned(), Vec::new())
        );
    }

    #[test]
    #[ignore = "needs a D-Bus session bus (run with dbus-run-session)"]
    fn falls_back_to_xdg_open_when_nothing_else_is_available() {
        let services = Services {
            file_manager: "org.everythux.test.AbsentFm",
            portal: "org.everythux.test.AbsentPortal",
            // `true` stands in for xdg-open so the test never opens a real window.
            xdg_open: "true",
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("f.txt");
        std::fs::write(&file, "x").expect("write");
        assert_eq!(
            show_item_with(&services, &file, "").expect("show_item"),
            Method::XdgOpen
        );
        assert_eq!(
            open_default_with(&services, &file, "").expect("open"),
            Method::XdgOpen
        );
    }
}
