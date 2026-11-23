use crate::models::AppStoreData;
use std::fs;
use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("antigravity-switch")
}

pub fn store_path() -> PathBuf {
    config_dir().join("store.json")
}

pub fn load_store() -> AppStoreData {
    let path = store_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return AppStoreData::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_store(data: &AppStoreData) -> Result<(), String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {e}"))?;
    let path = dir.join("store.json");
    let tmp = dir.join(format!("store.json.{}.tmp", std::process::id()));
    let json = serde_json::to_string_pretty(data).map_err(|e| format!("Serialize store failed: {e}"))?;

    fs::write(&tmp, &json).map_err(|e| format!("Write temp store failed: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, &path).map_err(|e| format!("Replace store failed: {e}"))?;
    Ok(())
}

pub fn normalize_email(raw: &str) -> String {
    raw.trim().to_lowercase()
}
