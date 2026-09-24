use std::path::PathBuf;

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
