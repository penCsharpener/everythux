//! Embeds the git identity of the build as `EVERYTHUX_BUILD_ID` (for example `ef0366a, modified`).
//!
//! Never fails the build: without git (a source tarball, say) the id is `unknown`.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    let id = match git(&["rev-parse", "--short", "HEAD"]).filter(|h| !h.is_empty()) {
        Some(hash) => {
            let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
            if dirty { format!("{hash}, modified") } else { hash }
        }
        None => "unknown".to_owned(),
    };
    println!("cargo:rustc-env=EVERYTHUX_BUILD_ID={id}");

    // Re-run when HEAD moves (worktree-safe: ask git where the git dir is) and when sources change,
    // so the dirty flag follows edits.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        let head = Path::new(&git_dir).join("HEAD");
        println!("cargo:rerun-if-changed={}", head.display());
        if let Ok(text) = std::fs::read_to_string(&head)
            && let Some(reference) = text.strip_prefix("ref: ")
            && let Some(common) = git(&["rev-parse", "--git-common-dir"])
        {
            let ref_file = Path::new(&common).join(reference.trim());
            println!("cargo:rerun-if-changed={}", ref_file.display());
        }
    }
}
