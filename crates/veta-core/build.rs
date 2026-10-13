//! Records the build number and commit for `veta_core::version`.
//!
//! The build number is `$VETA_BUILD_NUMBER` if set (e.g. by CI), otherwise
//! the number of commits on the checked-out branch. Without git, both are
//! left out.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=VETA_BUILD_NUMBER");
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        // A new commit or a branch switch changes these.
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/refs/heads");
        println!("cargo:rerun-if-changed={dir}/packed-refs");
    }
    let build = std::env::var("VETA_BUILD_NUMBER")
        .ok()
        .filter(|b| !b.is_empty())
        .or_else(|| git(&["rev-list", "--count", "HEAD"]))
        .unwrap_or_default();
    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=VETA_BUILD={build}");
    println!("cargo:rustc-env=VETA_COMMIT={commit}");
    let version = env!("CARGO_PKG_VERSION");
    let long = match (build.is_empty(), commit.is_empty()) {
        (false, false) => format!("{version} (build {build}, {commit})"),
        (false, true) => format!("{version} (build {build})"),
        _ => version.to_owned(),
    };
    println!("cargo:rustc-env=VETA_VERSION_LONG={long}");
}
