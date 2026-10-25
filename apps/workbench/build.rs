//! Windows resources of the executable (`docs/design/release.md` §4): the
//! icon, which GPUI loads as resource 1 for the window and the taskbar, and
//! the version information the file's properties show. Nothing elsewhere,
//! nor when cross-compiling for Windows (the executable then has no icon).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        #[cfg(windows)]
        windows::compile_resources();
    }
}

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    pub fn compile_resources() {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let icon = manifest_dir.join("../../packaging/icons/materials-workbench.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let version = std::env::var("CARGO_PKG_VERSION").unwrap();
        let number = |part: &str| std::env::var(part).unwrap();
        let numbers = format!(
            "{},{},{},0",
            number("CARGO_PKG_VERSION_MAJOR"),
            number("CARGO_PKG_VERSION_MINOR"),
            number("CARGO_PKG_VERSION_PATCH")
        );
        // The resource compiler takes `/` in paths, and `\` only escaped.
        let icon = icon.display().to_string().replace('\\', "/");
        // Without windows.h: 0x40004 is VOS_NT_WINDOWS32, 1 is VFT_APP, and
        // the translation is US English in UTF-16 (0x409, 1200).
        let script = format!(
            r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Materials Workbench"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "materials-workbench"
      VALUE "LegalCopyright", "MIT OR Apache-2.0"
      VALUE "OriginalFilename", "materials-workbench.exe"
      VALUE "ProductName", "Materials Workbench"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
        );
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("materials-workbench.rc");
        std::fs::write(&out, script).unwrap();
        // Only into the application: the tests and examples need no icon.
        embed_resource::compile_for(&out, ["materials-workbench"], embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
