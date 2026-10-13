//! Synthetic snapshots for tests and benchmarks. Not part of the stable API.
//!
//! [`SnapshotBuilder`] assembles small hand-written snapshots; [`generate`] produces a large,
//! seeded, realistic one (skewed names, extensions, folder sizes) without touching a filesystem.

use std::collections::HashMap;
use std::path::PathBuf;

use everythux_core::ids::{DirId, EntryId, VolumeId};
use everythux_core::model::{EntryKind, VolumeInfo, VolumeState};
use everythux_core::snapshot::IndexSnapshot;
use everythux_core::text::{extension, fold};

/// Small deterministic generator (xorshift64*), so no `rand` dependency is needed.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Seeds the generator (any value, including 0).
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0..n` (`n` > 0).
    pub fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Index in `0..n` skewed towards 0 (`power` > 1 skews harder).
    pub fn skewed(&mut self, n: usize, power: f64) -> usize {
        ((self.unit().powf(power) * n as f64) as usize).min(n - 1)
    }
}

/// Builds an [`IndexSnapshot`] entry by entry.
#[derive(Debug)]
pub struct SnapshotBuilder {
    snap: IndexSnapshot,
    ext_ids: HashMap<String, u16>,
    dir_by_path: HashMap<Vec<u8>, DirId>,
}

impl Default for SnapshotBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self {
            snap: IndexSnapshot::empty(),
            ext_ids: HashMap::new(),
            dir_by_path: HashMap::new(),
        }
    }

    /// Adds a volume.
    pub fn volume(&mut self, mount_point: &str, label: Option<&str>) -> VolumeId {
        let id = VolumeId(self.snap.volumes.len() as u32);
        self.snap.volumes.push(VolumeInfo {
            fs_uuid: format!("uuid-{}", id.0),
            device: PathBuf::from(format!("/dev/fake{}", id.0)),
            mount_point: PathBuf::from(mount_point),
            label: label.map(str::to_owned),
            fs_type: "ext4".to_owned(),
            state: VolumeState::Online,
            last_full_scan_ns: None,
            last_scan_ms: None,
        });
        id
    }

    /// Adds a folder row (no entry for it yet; see [`Self::folder_entry`]). `path` is absolute
    /// without trailing slash (`"/"` for a root).
    pub fn add_dir(&mut self, path: &[u8], parent: DirId, volume: VolumeId) -> DirId {
        let id = DirId(self.snap.dirs.len() as u32);
        self.snap.dirs.paths.push(path);
        self.snap.dirs.parent.push(parent);
        self.snap.dirs.volume.push(volume);
        self.snap.dirs.total_size.push(0);
        self.snap.dirs.mtime_ns.push(0);
        self.snap.dirs.entry.push(EntryId::NONE);
        self.dir_by_path.insert(path.to_vec(), id);
        id
    }

    /// Adds a folder at an absolute path, plus its entry when the parent folder is known. Returns
    /// the folder id.
    pub fn dir(&mut self, path: &str, volume: VolumeId) -> DirId {
        let bytes = path.as_bytes();
        let slash = bytes.iter().rposition(|&b| b == b'/').unwrap_or(0);
        let parent_path = if slash == 0 { &b"/"[..] } else { &bytes[..slash] };
        let parent = self
            .dir_by_path
            .get(parent_path)
            .copied()
            .filter(|_| bytes != b"/");
        let id = self.add_dir(bytes, parent.unwrap_or(DirId::NONE), volume);
        if let Some(parent) = parent {
            self.folder_entry(parent, id, &bytes[slash + 1..], 0);
        }
        id
    }

    /// Adds the entry that represents folder `child` inside folder `parent`.
    pub fn folder_entry(&mut self, parent: DirId, child: DirId, name: &[u8], mtime_ns: i64) -> EntryId {
        let id = self.entry(parent, name, EntryKind::Dir, 0, mtime_ns);
        self.snap.entries.self_dir[id.index()] = child;
        self.snap.dirs.entry[child.index()] = id;
        self.snap.dirs.mtime_ns[child.index()] = mtime_ns;
        id
    }

    /// Adds a regular file.
    pub fn file(&mut self, dir: DirId, name: &[u8], size: u64, mtime_ns: i64) -> EntryId {
        self.entry(dir, name, EntryKind::File, size, mtime_ns)
    }

    /// Adds an entry of any kind.
    pub fn entry(&mut self, dir: DirId, name: &[u8], kind: EntryKind, size: u64, mtime_ns: i64) -> EntryId {
        let e = &mut self.snap.entries;
        let id = EntryId(e.len() as u32);
        e.names.push(name);
        e.names_fold.push(fold(name).as_bytes());
        e.dir.push(dir);
        e.kind.push(kind);
        e.size.push(size);
        e.mtime_ns.push(mtime_ns);
        e.self_dir.push(DirId::NONE);
        let ext = match (kind, extension(name)) {
            (EntryKind::Dir, _) | (_, None) => 0,
            (_, Some(ext)) => self.intern_extension(&ext),
        };
        self.snap.entries.ext.push(ext);
        id
    }

    fn intern_extension(&mut self, ext: &str) -> u16 {
        if let Some(&id) = self.ext_ids.get(ext) {
            return id;
        }
        let id = self.snap.extensions.len() as u16;
        self.snap.extensions.push(ext.into());
        self.ext_ids.insert(ext.to_owned(), id);
        id
    }

    /// Computes folder sizes and returns the snapshot.
    pub fn finish(mut self) -> IndexSnapshot {
        let s = &mut self.snap;
        for i in 0..s.entries.len() {
            if s.entries.kind[i] != EntryKind::Dir {
                s.dirs.total_size[s.entries.dir[i].index()] += s.entries.size[i];
            }
        }
        // Parents are always created before their children, so one reverse pass accumulates.
        for d in (0..s.dirs.len()).rev() {
            let parent = s.dirs.parent[d];
            if !parent.is_none() {
                s.dirs.total_size[parent.index()] += s.dirs.total_size[d];
            }
        }
        for i in 0..s.entries.len() {
            let child = s.entries.self_dir[i];
            if !child.is_none() {
                s.entries.size[i] = s.dirs.total_size[child.index()];
            }
        }
        self.snap
    }
}

const WORDS: &[&str] = &[
    "report",
    "invoice",
    "holiday",
    "readme",
    "main",
    "test",
    "config",
    "backup",
    "photo",
    "document",
    "notes",
    "project",
    "design",
    "final",
    "draft",
    "meeting",
    "budget",
    "summary",
    "data",
    "image",
    "screenshot",
    "download",
    "music",
    "video",
    "movie",
    "archive",
    "old",
    "new",
    "copy",
    "version",
    "index",
    "util",
    "lib",
    "src",
    "build",
    "release",
    "debug",
    "temp",
    "cache",
    "log",
    "error",
    "result",
    "output",
    "input",
    "script",
    "module",
    "package",
    "setup",
    "install",
    "update",
    "system",
    "network",
    "server",
    "client",
    "user",
    "admin",
    "public",
    "private",
    "shared",
    "local",
    "remote",
    "global",
    "default",
    "custom",
    "template",
    "example",
    "sample",
    "demo",
    "tutorial",
    "guide",
    "manual",
    "reference",
    "spec",
    "schema",
    "model",
    "view",
    "controller",
    "service",
    "handler",
    "parser",
    "lexer",
    "token",
    "buffer",
    "stream",
    "socket",
    "thread",
    "mutex",
    "queue",
    "stack",
    "heap",
    "tree",
    "graph",
    "node",
    "edge",
    "list",
    "array",
    "table",
    "column",
    "row",
    "cell",
    "sheet",
    "page",
    "chapter",
    "section",
    "title",
    "header",
    "footer",
    "body",
    "content",
    "text",
    "string",
    "number",
    "integer",
    "float",
    "double",
    "boolean",
    "object",
    "class",
    "struct",
    "enum",
    "trait",
    "impl",
    "type",
    "alpha",
    "beta",
    "gamma",
    "delta",
    "epsilon",
    "omega",
    "sigma",
    "kappa",
    "lambda",
    "theta",
    "paris",
    "london",
    "berlin",
    "tokyo",
    "madrid",
    "rome",
    "vienna",
    "prague",
    "oslo",
    "lisbon",
    "caf\u{e9}",
    "\u{fc}bung",
    "stra\u{df}e",
    "\u{65e5}\u{672c}\u{8a9e}",
    "\u{43f}\u{440}\u{438}\u{43c}\u{435}\u{440}",
];

const DIR_WORDS: &[&str] = &[
    "src",
    "lib",
    "bin",
    "docs",
    "tests",
    "target",
    "node_modules",
    "assets",
    "images",
    "build",
    "include",
    "share",
    "etc",
    "var",
    "tmp",
    "cache",
    "logs",
    "backup",
    "projects",
    "photos",
    "music",
    "videos",
    "Downloads",
    "Documents",
    "Pictures",
    "work",
    "personal",
    "archive",
    "old",
    "misc",
    "data",
    "config",
    "plugins",
    "vendor",
];

/// Extensions with relative weights; `""` means no extension.
const EXTENSIONS: &[(&str, u32)] = &[
    ("jpg", 12),
    ("png", 8),
    ("txt", 7),
    ("rs", 3),
    ("c", 3),
    ("h", 3),
    ("py", 4),
    ("js", 5),
    ("json", 4),
    ("md", 4),
    ("pdf", 4),
    ("html", 3),
    ("css", 2),
    ("so", 2),
    ("o", 2),
    ("log", 3),
    ("xml", 2),
    ("gz", 2),
    ("mp3", 2),
    ("mp4", 1),
    ("zip", 1),
    ("docx", 2),
    ("odt", 1),
    ("svg", 2),
    ("toml", 1),
    ("yml", 1),
    ("sh", 1),
    ("cpp", 2),
    ("java", 2),
    ("class", 4),
    ("pyc", 3),
    ("", 3),
];

const MTIME_BASE_NS: i64 = 1_760_000_000 * 1_000_000_000;
const FIVE_YEARS_S: u64 = 5 * 365 * 86_400;

fn pick_ext(rng: &mut Rng) -> &'static str {
    let total: u32 = EXTENSIONS.iter().map(|e| e.1).sum();
    let mut roll = rng.below(total as usize) as u32;
    for &(ext, weight) in EXTENSIONS {
        if roll < weight {
            return ext;
        }
        roll -= weight;
    }
    "txt"
}

fn word(rng: &mut Rng) -> &'static str {
    WORDS[rng.skewed(WORDS.len(), 2.2)]
}

fn capitalised(w: &str) -> String {
    let mut chars = w.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// A realistic file name; roughly 1 in 20 000 contains the rare token `qzx`, and a handful of names
/// are not valid UTF-8.
fn file_name(rng: &mut Rng) -> Vec<u8> {
    let ext = pick_ext(rng);
    let dot_ext = if ext.is_empty() {
        String::new()
    } else {
        format!(".{ext}")
    };
    let mut name = match rng.below(100) {
        0..30 => format!("{}_{}{dot_ext}", word(rng), word(rng)),
        30..50 => format!("{}-{}{dot_ext}", word(rng), rng.below(100_000)),
        50..62 => format!("IMG_{:04}.jpg", rng.below(10_000)),
        62..78 => format!("{}{}{dot_ext}", capitalised(word(rng)), capitalised(word(rng))),
        78..88 => format!("{}{dot_ext}", word(rng)),
        88..94 => format!(
            "{} {} ({}){dot_ext}",
            word(rng),
            capitalised(word(rng)),
            rng.below(20)
        ),
        94..97 => format!(".{}rc", word(rng)),
        _ => format!("{}.tar.gz", word(rng)),
    };
    if rng.below(20_000) == 0 {
        name = format!("{name}.qzx{}", rng.below(1000));
    }
    let mut bytes = name.into_bytes();
    if rng.below(2_000) == 0 {
        bytes.insert(rng.below(bytes.len() + 1), 0xff);
    }
    bytes
}

/// Generates a snapshot with exactly `entries` entries (files and folders) in about `entries / 20`
/// folders over two volumes (`/` and `/mnt/data`), deterministically from `seed`.
pub fn generate(entries: usize, seed: u64) -> IndexSnapshot {
    let mut rng = Rng::new(seed);
    let mut b = SnapshotBuilder::new();
    let root_vol = b.volume("/", None);
    let data_vol = b.volume("/mnt/data", Some("data"));

    let ndirs = (entries / 20).clamp(2, entries.max(2));
    let mut paths: Vec<Vec<u8>> = vec![b"/".to_vec(), b"/mnt/data".to_vec()];
    let mut depth: Vec<u32> = vec![0, 0];
    let mut volume_of: Vec<VolumeId> = vec![root_vol, data_vol];
    let mut children: Vec<Vec<DirId>> = vec![Vec::new(), Vec::new()];
    b.add_dir(b"/", DirId::NONE, root_vol);
    b.add_dir(b"/mnt/data", DirId::NONE, data_vol);
    let mut dir_names: Vec<Vec<u8>> = vec![Vec::new(), Vec::new()];

    for d in 2..ndirs {
        // Half of the folders hang off a random earlier folder, half extend a recent chain.
        let parent = loop {
            let p = if rng.below(2) == 0 {
                rng.below(d)
            } else {
                d - 1 - rng.below(d.min(48))
            };
            if depth[p] < 12 {
                break p;
            }
        };
        let mut name = DIR_WORDS[rng.skewed(DIR_WORDS.len(), 1.5)].as_bytes().to_vec();
        if rng.below(3) == 0 {
            name.extend_from_slice(rng.below(1000).to_string().as_bytes());
        }
        let mut path = paths[parent].clone();
        if path != b"/" {
            path.push(b'/');
        }
        path.extend_from_slice(&name);
        let id = b.add_dir(&path, DirId(parent as u32), volume_of[parent]);
        children[parent].push(id);
        paths.push(path);
        depth.push(depth[parent] + 1);
        volume_of.push(volume_of[parent]);
        children.push(Vec::new());
        dir_names.push(name);
    }

    // Files per folder: heavily skewed, summing to exactly the rest of the entries.
    let files_total = entries.saturating_sub(ndirs - 2);
    let weights: Vec<f64> = (0..ndirs).map(|_| rng.unit().powi(3) + 0.002).collect();
    let weight_sum: f64 = weights.iter().sum();
    let mut counts: Vec<usize> = weights
        .iter()
        .map(|w| (w / weight_sum * files_total as f64) as usize)
        .collect();
    let mut assigned: usize = counts.iter().sum();
    while assigned < files_total {
        counts[rng.below(ndirs)] += 1;
        assigned += 1;
    }

    for d in 0..ndirs {
        let dir = DirId(d as u32);
        for &child in &children[d] {
            let mtime = MTIME_BASE_NS - (rng.next_u64() % FIVE_YEARS_S).cast_signed() * 1_000_000_000;
            b.folder_entry(dir, child, &dir_names[child.index()], mtime);
        }
        for _ in 0..counts[d] {
            let name = file_name(&mut rng);
            let size = if rng.below(33) == 0 {
                0
            } else {
                ((rng.below(1024) + 1) << rng.below(20)) as u64 >> 3
            };
            let mtime = MTIME_BASE_NS - ((rng.next_u64() % FIVE_YEARS_S) * 1_000_000_000).cast_signed();
            b.file(dir, &name, size, mtime);
        }
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_is_deterministic_and_exact() {
        let a = generate(5_000, 7);
        let b = generate(5_000, 7);
        assert_eq!(a.len(), 5_000);
        assert_eq!(a.entries.names.bytes(), b.entries.names.bytes());
        assert_eq!(a.dirs.paths.bytes(), b.dirs.paths.bytes());
        let c = generate(5_000, 8);
        assert_ne!(a.entries.names.bytes(), c.entries.names.bytes());
    }

    #[test]
    fn generated_snapshot_is_consistent() {
        let s = generate(20_000, 1);
        for i in 0..s.len() {
            assert!(s.entries.dir[i].index() < s.dirs.len());
            let child = s.entries.self_dir[i];
            assert_eq!(!child.is_none(), s.entries.kind[i] == EntryKind::Dir);
            if !child.is_none() {
                assert_eq!(s.dirs.entry[child.index()].index(), i);
            }
            assert_eq!(
                s.entries.names_fold.get(i),
                fold(s.entries.names.get(i)).as_bytes()
            );
        }
        assert!(s.extensions.len() > 10);
        assert!(s.dirs.total_size[0] > 0);
    }

    #[test]
    fn builder_resolves_parents() {
        let mut b = SnapshotBuilder::new();
        let v = b.volume("/", None);
        let root = b.dir("/", v);
        let home = b.dir("/home", v);
        let f = b.file(home, b"a.TXT", 10, 5);
        let s = b.finish();
        assert_eq!(s.dirs.parent[home.index()], root);
        assert_eq!(s.extension(f), "txt");
        assert_eq!(s.dirs.total_size[root.index()], 10);
        assert_eq!(s.full_path(f), PathBuf::from("/home/a.TXT"));
        assert_eq!(s.entries.size[s.dirs.entry[home.index()].index()], 10);
    }
}
