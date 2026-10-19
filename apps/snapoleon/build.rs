//! Windows only: embeds the application icon, version info and manifest into the executable.

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets = manifest_dir.join("assets");
    let rc_path = |name: &str| assets.join(name).display().to_string().replace('\\', "/");

    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor, patch) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));

    let rc = format!(
        r#"1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "ProductName", "Snapoleon"
      VALUE "FileDescription", "Snapoleon"
      VALUE "InternalName", "snapoleon"
      VALUE "OriginalFilename", "snapoleon.exe"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "LegalCopyright", "MIT OR Apache-2.0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = rc_path("snapoleon.ico"),
        manifest = rc_path("snapoleon.manifest"),
    );

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("snapoleon.rc");
    fs::write(&out, rc).expect("write resource script");
    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}
