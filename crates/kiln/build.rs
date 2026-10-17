//! Embeds the icon, version information and application manifest in Windows executables.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=windows/kiln.manifest");
    println!("cargo:rerun-if-changed=../../assets/Kiln.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numbers: Vec<u16> = version.split(['.', '-']).take(3).map(|part| part.parse().unwrap_or(0)).collect();
    let number = format!("{},{},{},0", numbers[0], numbers[1], numbers[2]);
    let quoted = |path: PathBuf| path.to_string_lossy().replace('\\', "\\\\");
    let rc = format!(
        r#"1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {number}
PRODUCTVERSION {number}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Kiln"
      VALUE "FileDescription", "Kiln"
      VALUE "FileVersion", "{version}"
      VALUE "LegalCopyright", "MIT License"
      VALUE "ProductName", "Kiln"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = quoted(dir.join("../../assets/Kiln.ico")),
        manifest = quoted(dir.join("windows/kiln.manifest")),
    );
    let path = out.join("kiln.rc");
    fs::write(&path, rc).unwrap();
    embed_resource::compile(&path, embed_resource::NONE).manifest_required().unwrap();
}
