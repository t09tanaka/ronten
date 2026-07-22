use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

fn main() {
    // Build identity (Task 5.4 / P1-12): embed enough about *this specific
    // binary* that two builds both claiming `ronten_version` 0.1.0 are still
    // distinguishable. Runs unconditionally, before the frontend-build branch
    // below, so it applies to a packaged-crate build too (no git checkout,
    // but TARGET/PROFILE/rustc are still known).
    emit_build_identity();

    // Published crates contain the prebuilt `frontend/dist` but none of the
    // frontend sources (see `include` in Cargo.toml). In that case there is
    // nothing to build — and running npm here would modify the package
    // source tree, which `cargo package`/`cargo publish` verification
    // rejects.
    if !Path::new("frontend/package.json").exists() {
        assert!(
            Path::new("frontend/dist/index.html").exists(),
            "frontend/dist is missing from this package; it must be built \
             (cargo build in a git checkout) before packaging"
        );
        println!("cargo:rerun-if-changed=frontend/dist");
        emit_frontend_digest();
        return;
    }
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/public");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/package-lock.json");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");
    println!("cargo:rerun-if-changed=frontend/svelte.config.js");
    println!("cargo:rerun-if-changed=frontend/tsconfig.json");
    println!("cargo:rerun-if-changed=frontend/tsconfig.app.json");
    println!("cargo:rerun-if-changed=frontend/tsconfig.node.json");
    let dist = Path::new("frontend/dist");
    if std::env::var_os("RONTEN_SKIP_FRONTEND_BUILD").is_some() && dist.join("index.html").exists()
    {
        // The skip flag is a deliberate escape hatch (e.g. iterating on Rust
        // code without paying the npm build cost), but a dist left over from
        // an earlier, now-stale frontend checkout would silently ship stale
        // UI. Warn (never hard-fail — that would defeat the point of the
        // escape hatch).
        warn_if_frontend_stale(dist);
        emit_frontend_digest();
        return;
    }
    if needs_install() {
        run("npm", &["ci", "--no-audit", "--no-fund"]);
        write_stamp();
    }
    run("npm", &["run", "build"]);
    emit_frontend_digest();
}

/// Emits `cargo:rustc-env` vars consumed by `BuildInfo::current()`
/// (`src/model.rs`) via `option_env!`. Every one of these is best-effort:
/// none of them may fail the build, since a build outside a git checkout (or
/// without a resolvable `rustc`) must still compile and run — it just gets
/// `None` for that field.
fn emit_build_identity() {
    // Re-run when the checked-out commit changes, so a rebuild after
    // `git commit`/`git checkout` picks up the new commit/dirty state
    // (`option_env!` is resolved at compile time). Best-effort: a build from
    // an extracted source tarball (no `.git`) just skips this.
    if Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
        if let Ok(head) = std::fs::read_to_string(".git/HEAD") {
            if let Some(ref_path) = head.trim().strip_prefix("ref: ") {
                let ref_file = Path::new(".git").join(ref_path);
                if ref_file.exists() {
                    println!("cargo:rerun-if-changed={}", ref_file.display());
                }
            }
        }
    }

    if let Some(commit) = git_output(&["rev-parse", "HEAD"]) {
        println!("cargo:rustc-env=RONTEN_SOURCE_COMMIT={commit}");
    }
    if let Some(status) = git_output(&["status", "--porcelain"]) {
        let dirty = !status.is_empty();
        println!("cargo:rustc-env=RONTEN_SOURCE_DIRTY={dirty}");
    }

    if let Some(version) = rustc_version() {
        println!("cargo:rustc-env=RONTEN_RUSTC_VERSION={version}");
    }

    // TARGET/PROFILE are build-script-only env vars (not available to the
    // compiled binary at runtime), so they must be forwarded explicitly.
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=RONTEN_TARGET={target}");
    }
    if let Ok(profile) = std::env::var("PROFILE") {
        println!("cargo:rustc-env=RONTEN_PROFILE={profile}");
    }
}

/// Runs `git <args>` and returns trimmed stdout, or `None` on any failure
/// (git missing, not a repo, non-UTF-8 output, ...). Never panics — git
/// identity is a nice-to-have, not a build requirement.
fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}

/// `rustc --version` output, via the `RUSTC` env cargo sets (falling back to
/// `rustc` on `PATH`). `None` on any failure.
fn rustc_version() -> Option<String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = Command::new(rustc).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Ties the binary to the exact embedded frontend artifact: SHA-256 over the
/// sorted `(relative_path, file_bytes)` pairs of every file under
/// `frontend/dist`, each pair separated by a NUL byte after the path. Sorting
/// the relative paths (forward-slash-normalized) before hashing makes the
/// digest independent of filesystem iteration order and of the host OS's
/// path separator, so the same dist contents always produce the same digest.
/// A missing dist (shouldn't happen by the time this runs) just leaves
/// `RONTEN_FRONTEND_DIGEST` unset.
fn emit_frontend_digest() {
    let dist = Path::new("frontend/dist");
    if !dist.exists() {
        return;
    }
    let mut files = Vec::new();
    collect_files(dist, dist, &mut files);
    files.sort();

    let mut hasher = Sha256::new();
    for rel in &files {
        let Ok(bytes) = std::fs::read(dist.join(rel)) else {
            continue;
        };
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(&bytes);
    }
    let digest = hasher.finalize();
    println!("cargo:rustc-env=RONTEN_FRONTEND_DIGEST={digest:x}");
}

/// Recursively collects file paths under `dir`, relative to `root`, as
/// forward-slash strings (so the digest is stable across platforms).
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// P1-9 (middle): `RONTEN_SKIP_FRONTEND_BUILD` is a deliberate escape hatch
/// that trusts whatever `frontend/dist` already contains, so it's easy to
/// forget it's set and ship a stale UI after editing frontend sources. Warns
/// (never fails the build) when `dist`'s newest file is older than the
/// newest file under `frontend/src` or `frontend/package-lock.json`.
fn warn_if_frontend_stale(dist: &Path) {
    let Some(dist_time) = newest_mtime(dist) else {
        return;
    };
    let src_time = newest_mtime(Path::new("frontend/src"));
    let lock_time = file_mtime(Path::new("frontend/package-lock.json"));
    let newest_source = match (src_time, lock_time) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    if let Some(newest_source) = newest_source {
        if dist_time < newest_source {
            println!(
                "cargo:warning=RONTEN_SKIP_FRONTEND_BUILD is set and frontend/dist looks \
                 older than frontend/src or frontend/package-lock.json; the embedded \
                 frontend may be stale"
            );
        }
    }
}

/// Newest modification time of `path` itself (if a file) or of any file
/// reachable under it (if a directory), recursively. `None` if `path` is
/// missing or unreadable.
fn newest_mtime(path: &Path) -> Option<SystemTime> {
    if path.is_file() {
        return file_mtime(path);
    }
    let entries = std::fs::read_dir(path).ok()?;
    let mut newest: Option<SystemTime> = None;
    for entry in entries.flatten() {
        let candidate_path = entry.path();
        let candidate = if candidate_path.is_dir() {
            newest_mtime(&candidate_path)
        } else {
            file_mtime(&candidate_path)
        };
        if let Some(c) = candidate {
            newest = Some(newest.map_or(c, |n| n.max(c)));
        }
    }
    newest
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Install dependencies when `node_modules` is missing or `package-lock.json`
/// has changed since the last install (tracked via a hash stamp in OUT_DIR).
fn needs_install() -> bool {
    if !Path::new("frontend/node_modules").exists() {
        return true;
    }
    match std::fs::read_to_string(stamp_path()) {
        Ok(stamp) => stamp != lockfile_hash(),
        Err(_) => true,
    }
}

fn write_stamp() {
    let path = stamp_path();
    std::fs::write(&path, lockfile_hash())
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
}

fn stamp_path() -> std::path::PathBuf {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    Path::new(&out_dir).join("npm-install.stamp")
}

fn lockfile_hash() -> String {
    let lockfile = std::fs::read("frontend/package-lock.json")
        .unwrap_or_else(|e| panic!("failed to read frontend/package-lock.json: {e}"));
    let mut hasher = DefaultHasher::new();
    lockfile.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn run(cmd: &str, args: &[&str]) {
    let status = Command::new(cmd)
        .args(args)
        .current_dir("frontend")
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn `{cmd}` (is Node.js installed?): {e}"));
    if !status.success() {
        panic!("`{cmd} {}` failed with {status}", args.join(" "));
    }
}
