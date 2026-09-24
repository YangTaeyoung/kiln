use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;

/// JSON 파일을 읽는다. 파일이 없거나 파싱에 실패하면 기본값을 돌려준다.
pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// 임시 파일에 쓴 뒤 rename 해서 원자적으로 저장한다.
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
