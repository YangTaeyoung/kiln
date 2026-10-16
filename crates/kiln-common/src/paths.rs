use std::{io, path::{Path, PathBuf}};

/// 설정 디렉토리. `KILN_CONFIG_DIR` 환경변수가 있으면 그 경로를 쓴다.
pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("KILN_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    directories::ProjectDirs::from("dev", "kiln", "kiln")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("kiln-config"))
}

pub fn config_file(name: &str) -> PathBuf {
    config_dir().join(name)
}

/// `std::fs::canonicalize` without the `\\?\` prefix on Windows drive and UNC paths.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    Ok(simplified(&canonical).into_owned())
}

/// [`canonicalize`] as a method of paths.
pub trait Canonical {
    fn canonical(&self) -> io::Result<PathBuf>;
}
impl Canonical for Path {
    fn canonical(&self) -> io::Result<PathBuf> {
        canonicalize(self)
    }
}

/// `path` without the `\\?\` prefix when it is a Windows drive or UNC path whose plain
/// form names the same file.
pub fn simplified(path: &Path) -> std::borrow::Cow<'_, Path> {
    let plain = dunce::simplified(path);
    if plain != path {
        return plain.into();
    }
    #[cfg(windows)]
    if let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\UNC\")) {
        if text_is_plain(rest) {
            return PathBuf::from(format!(r"\\{rest}")).into();
        }
    }
    path.into()
}

/// Whether every component of a `\\`-separated path keeps its meaning without the
/// verbatim prefix: no reserved device names, no trailing dots or spaces, and a total
/// length under `MAX_PATH`.
#[cfg(windows)]
fn text_is_plain(rest: &str) -> bool {
    const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    rest.len() + 2 < 260
        && rest.split('\\').all(|part| {
            let stem = part.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
            let device = RESERVED.contains(&stem.as_str())
                || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
            !part.is_empty() && !part.ends_with(['.', ' ']) && !device && !part.contains('/')
        })
}

/// The user's home folder: `HOME` on Unix and `USERPROFILE` on Windows, else the
/// system's record of the profile folder.
pub fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()))
}

/// The part of `raw` after a leading `~` and separator (`~/`, and `~\` on Windows), or
/// `""` for `~` alone; `None` when `raw` does not start with the home folder.
pub fn tilde_rest(raw: &str) -> Option<&str> {
    if raw == "~" {
        return Some("");
    }
    raw.strip_prefix("~/").or_else(|| if cfg!(windows) { raw.strip_prefix("~\\") } else { None })
}

/// Finds a program in `PATH`, then in `extra`. A name with more than one path component
/// is checked as given. On Windows a name without a `PATHEXT` extension gets each
/// `PATHEXT` extension in order and extension-less files never match; on Unix the file
/// must be executable.
pub fn find_executable(name: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let exts = extensions();
    let path = Path::new(name);
    if path.components().count() > 1 {
        return candidates(path, &exts).into_iter().find(|p| runnable(p));
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(extra.iter().cloned());
    lookup(name, &dirs, &exts)
}

/// [`find_executable`] limited to `dirs`, without `PATH`.
pub fn find_executable_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    lookup(name, dirs, &extensions())
}

fn extensions() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    std::env::var("PATHEXT")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|ext| ext.starts_with('.') && ext.len() > 1)
        .map(str::to_owned)
        .collect()
}

fn lookup(name: &str, dirs: &[PathBuf], exts: &[String]) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| candidates(&dir.join(name), exts).into_iter().find(|p| runnable(p)))
}

/// The paths `path` may name: itself on Unix; on Windows itself when its extension is in
/// `exts`, otherwise `path` with each of `exts` appended in lowercase.
fn candidates(path: &Path, exts: &[String]) -> Vec<PathBuf> {
    if !cfg!(windows) {
        return vec![path.to_path_buf()];
    }
    let listed = path.extension().is_some_and(|ext| {
        exts.iter().any(|known| known[1..].eq_ignore_ascii_case(&ext.to_string_lossy()))
    });
    if listed {
        return vec![path.to_path_buf()];
    }
    exts.iter()
        .map(|ext| {
            let mut name = path.as_os_str().to_owned();
            name.push(ext.to_ascii_lowercase());
            PathBuf::from(name)
        })
        .collect()
}

fn runnable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_paths_have_no_verbatim_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = canonicalize(dir.path()).unwrap();
        assert!(!path.to_string_lossy().starts_with(r"\\?\"), "{}", path.display());
        assert!(path.is_absolute());
        assert_eq!(std::fs::canonicalize(&path).unwrap(), std::fs::canonicalize(dir.path()).unwrap());
    }

    #[test]
    fn canonical_paths_keep_unc_and_unicode_names() {
        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join("한글 폴더");
        std::fs::create_dir(&named).unwrap();
        assert_eq!(canonicalize(&named).unwrap().file_name().unwrap(), "한글 폴더");
        #[cfg(windows)]
        {
            assert_eq!(simplified(Path::new(r"\\?\UNC\srv\share\x")), Path::new(r"\\srv\share\x"));
            assert_eq!(simplified(Path::new(r"\\?\C:\a b\c")), Path::new(r"C:\a b\c"));
        }
    }

    #[test]
    fn tilde_rest_accepts_the_platform_separators() {
        assert_eq!(tilde_rest("~"), Some(""));
        assert_eq!(tilde_rest("~/a/b"), Some("a/b"));
        assert_eq!(tilde_rest("~user/a"), None);
        assert_eq!(tilde_rest("/a/~"), None);
        assert_eq!(tilde_rest("~\\a"), if cfg!(windows) { Some("a") } else { None });
    }

    #[test]
    fn home_is_an_existing_folder() {
        assert!(home().unwrap().is_dir());
    }

    #[test]
    fn executable_lookup_checks_explicit_paths_directly() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_executable(&dir.path().join("missing").to_string_lossy(), &[]), None);
    }

    #[cfg(unix)]
    #[test]
    fn executable_lookup_requires_the_execute_bit() {
        use std::os::unix::fs::PermissionsExt;
        let plain = tempfile::tempdir().unwrap();
        let runnable = tempfile::tempdir().unwrap();
        for (dir, mode) in [(&plain, 0o600), (&runnable, 0o700)] {
            let path = dir.path().join("tool");
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let dirs = [plain.path().to_path_buf(), runnable.path().to_path_buf()];
        assert_eq!(lookup("tool", &dirs, &[]), Some(runnable.path().join("tool")));
    }

    #[cfg(windows)]
    #[test]
    fn executable_lookup_follows_pathext_order_and_skips_extensionless_shims() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["tool", "tool.cmd", "tool.exe"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let dirs = [dir.path().to_path_buf()];
        let exts = |list: &[&str]| list.iter().map(|e| e.to_string()).collect::<Vec<_>>();
        assert_eq!(lookup("tool", &dirs, &exts(&[".EXE", ".CMD"])), Some(dir.path().join("tool.exe")));
        assert_eq!(lookup("tool", &dirs, &exts(&[".CMD", ".EXE"])), Some(dir.path().join("tool.cmd")));
        assert_eq!(lookup("tool.cmd", &dirs, &exts(&[".EXE", ".CMD"])), Some(dir.path().join("tool.cmd")));
        assert_eq!(lookup("tool", &dirs, &exts(&[".BAT"])), None);
        assert!(Path::new(&lookup("tool", &dirs, &exts(&[".EXE"])).unwrap()).extension().is_some());
    }
}
