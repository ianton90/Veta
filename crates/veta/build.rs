//! On Windows, embeds the app icon and version information into `veta.exe`
//! (Explorer shows them; Properties → Details lists the version and build
//! number). The build number is `$VETA_BUILD_NUMBER` or the commit count,
//! as in `veta_core::version`.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=VETA_BUILD_NUMBER");
    println!("cargo:rerun-if-changed=../../packaging/icons/veta.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env("CARGO_PKG_VERSION");
    let part = |name: &str| env(name).parse::<u16>().unwrap_or(0);
    let (major, minor, patch) = (
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
    );
    let build: u16 = std::env::var("VETA_BUILD_NUMBER")
        .ok()
        .or_else(commit_count)
        .and_then(|b| b.trim().parse().ok())
        .unwrap_or(0);
    let icon = Path::new(&env("CARGO_MANIFEST_DIR")).join("../../packaging/icons/veta.ico");
    let icon = icon.to_string_lossy().replace('\\', "/");
    let rc = format!(
        r#"#include <winver.h>
1 ICON "{icon}"
VS_VERSION_INFO VERSIONINFO
FILEVERSION {major},{minor},{patch},{build}
PRODUCTVERSION {major},{minor},{patch},{build}
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Veta"
      VALUE "FileDescription", "Veta - Parquet viewer and editor"
      VALUE "FileVersion", "{version}.{build}"
      VALUE "InternalName", "veta"
      VALUE "LegalCopyright", "Apache-2.0"
      VALUE "OriginalFilename", "veta.exe"
      VALUE "ProductName", "Veta"
      VALUE "ProductVersion", "{version} (build {build})"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let path = Path::new(&env("OUT_DIR")).join("veta.rc");
    std::fs::write(&path, rc).expect("write veta.rc");
    embed_resource::compile(&path, embed_resource::NONE)
        .manifest_optional()
        .expect("compile veta.rc");
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn commit_count() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}
