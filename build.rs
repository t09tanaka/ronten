use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");
    let dist = Path::new("frontend/dist");
    if std::env::var_os("RONTEN_SKIP_FRONTEND_BUILD").is_some() && dist.join("index.html").exists()
    {
        return;
    }
    if !Path::new("frontend/node_modules").exists() {
        run("npm", &["install", "--no-audit", "--no-fund"]);
    }
    run("npm", &["run", "build"]);
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
