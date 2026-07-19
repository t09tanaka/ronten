use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=frontend/src");
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
        return;
    }
    if needs_install() {
        run("npm", &["ci", "--no-audit", "--no-fund"]);
        write_stamp();
    }
    run("npm", &["run", "build"]);
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
