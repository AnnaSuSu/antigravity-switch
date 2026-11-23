use crate::protobuf::{
    parse_oauth_token_topic_b64, parse_user_status_topic_b64, patch_oauth_token_topic_b64,
    patch_user_status_topic_b64, UnifiedOAuthToken,
};
use crate::store::normalize_email;
use base64::{engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL}, Engine};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const OAUTH_SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform \
https://www.googleapis.com/auth/userinfo.email \
https://www.googleapis.com/auth/userinfo.profile \
https://www.googleapis.com/auth/cclog \
https://www.googleapis.com/auth/experimentsandconfigs";

const DB_KEYS: &[&str] = &[
    "antigravityUnifiedStateSync.oauthToken",
    "antigravityUnifiedStateSync.userStatus",
    "antigravityOnboarding",
];

#[derive(Debug, Clone)]
pub struct CredentialPaths {
    pub desktop_db: PathBuf,
    pub ide_db: PathBuf,
    pub google_accounts: PathBuf,
    pub oauth_creds: PathBuf,
    pub jetski_token: PathBuf,
    pub cli_token: PathBuf,
}

impl CredentialPaths {
    pub fn detect() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let app_support = {
            #[cfg(target_os = "macos")]
            {
                home.join("Library/Application Support")
            }
            #[cfg(target_os = "windows")]
            {
                std::env::var("APPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| home.join("AppData/Roaming"))
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                dirs::config_dir().unwrap_or_else(|| home.join(".config"))
            }
        };

        Self {
            desktop_db: app_support.join("Antigravity/User/globalStorage/state.vscdb"),
            ide_db: app_support.join("Antigravity IDE/User/globalStorage/state.vscdb"),
            google_accounts: home.join(".gemini/google_accounts.json"),
            oauth_creds: home.join(".gemini/oauth_creds.json"),
            jetski_token: home.join(".gemini/jetski-standalone-oauth-token"),
            cli_token: home.join(".gemini/antigravity-cli/antigravity-oauth-token"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SystemCredentialInput {
    pub email: String,
    pub name: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
    pub id_token: String,
}

#[derive(Debug, Clone)]
pub struct ImportedCredential {
    pub email: String,
    pub name: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
    pub id_token: String,
}

#[derive(Debug, Clone)]
pub struct CredentialSnapshot {
    pub os_cred_raw: Option<String>,
    pub desktop_db_rows: HashMap<String, Option<String>>,
    pub ide_db_rows: HashMap<String, Option<String>>,
    pub google_accounts_raw: Option<String>,
    pub oauth_creds_raw: Option<String>,
    pub jetski_token_raw: Option<String>,
    pub cli_token_raw: Option<String>,
}

pub fn decode_jwt_email(id_token: &str) -> String {
    let Some(payload_b64) = id_token.split('.').nth(1) else {
        return String::new();
    };
    let Ok(bytes) = B64URL.decode(payload_b64) else {
        return String::new();
    };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
        return String::new();
    };
    v.get("email")
        .and_then(|e| e.as_str())
        .map(normalize_email)
        .unwrap_or_default()
}

// ---------------- OS Keychain / Credential Manager ----------------

#[cfg(target_os = "macos")]
pub fn read_os_credential_raw() -> Option<String> {
    let out = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", "gemini", "-a", "antigravity", "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if let Some(b64) = raw.strip_prefix("go-keyring-base64:") {
        let decoded = B64.decode(b64).ok()?;
        Some(String::from_utf8_lossy(&decoded).into_owned())
    } else if !raw.is_empty() {
        Some(raw)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
pub fn write_os_credential_raw(json_str: &str) -> Result<(), String> {
    let encoded = format!("go-keyring-base64:{}", B64.encode(json_str.as_bytes()));
    let out = std::process::Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            "gemini",
            "-a",
            "antigravity",
            "-w",
            &encoded,
        ])
        .output()
        .map_err(|e| format!("调用 macOS Keychain 失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "写入 macOS Keychain 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn read_os_credential_raw() -> Option<String> {
    use windows_sys::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };
    let target: Vec<u16> = "gemini:antigravity\0".encode_utf16().collect();
    let mut ptr: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut ptr) == 0 || ptr.is_null() {
            return None;
        }
        let cred = &*ptr;
        let result = if cred.CredentialBlobSize > 0 && !cred.CredentialBlob.is_null() {
            let slice = std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize);
            Some(String::from_utf8_lossy(slice).into_owned())
        } else {
            None
        };
        CredFree(ptr as *const _);
        result
    }
}

#[cfg(target_os = "windows")]
pub fn write_os_credential_raw(json_str: &str) -> Result<(), String> {
    use windows_sys::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };
    let mut target: Vec<u16> = "gemini:antigravity\0".encode_utf16().collect();
    let mut user: Vec<u16> = "antigravity\0".encode_utf16().collect();
    let mut blob = json_str.as_bytes().to_vec();

    let cred = CREDENTIALW {
        Flags: 0,
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_mut_ptr(),
        Comment: std::ptr::null_mut(),
        LastWritten: unsafe { std::mem::zeroed() },
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: std::ptr::null_mut(),
        UserName: user.as_mut_ptr(),
    };

    let ok = unsafe { CredWriteW(&cred, 0) };
    if ok == 0 {
        return Err("写入 Windows Credential Manager 失败".into());
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn read_os_credential_raw() -> Option<String> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn write_os_credential_raw(_json_str: &str) -> Result<(), String> {
    Ok(())
}

// ---------------- SQLite state.vscdb helpers ----------------

fn read_state_db_items(db_path: &Path) -> HashMap<String, Option<String>> {
    let mut out = HashMap::new();
    if !db_path.exists() {
        return out;
    }
    let Ok(conn) = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return out;
    };
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    for &key in DB_KEYS {
        let val = conn
            .query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .ok();
        out.insert(key.to_string(), val);
    }
    out
}

fn write_state_db_items(db_path: &Path, entries: &HashMap<String, Option<String>>) -> Result<(), String> {
    if !db_path.exists() {
        return Ok(());
    }
    let mut conn = Connection::open(db_path).map_err(|e| format!("打开 state.vscdb 失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("设置 SQLite busy_timeout 失败: {e}"))?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| format!("开启 state.vscdb 事务失败: {e}"))?;
    for (k, v) in entries {
        match v {
            Some(val) => {
                tx.execute(
                    "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
                    [k.as_str(), val.as_str()],
                )
                .map_err(|e| format!("写入 state.vscdb 键 {k} 失败: {e}"))?;
            }
            None => {
                tx.execute("DELETE FROM ItemTable WHERE key = ?1", [k.as_str()])
                    .map_err(|e| format!("删除 state.vscdb 键 {k} 失败: {e}"))?;
            }
        }
    }
    tx.commit().map_err(|e| format!("提交 state.vscdb 事务失败: {e}"))?;
    Ok(())
}

/// Atomically reads existing UnifiedStateSync topics and patches only the OAuth/UserStatus
/// sentinel entries inside one `BEGIN IMMEDIATE` transaction.
fn patch_state_db_credentials(
    db_path: &Path,
    token: &UnifiedOAuthToken,
    email: &str,
    name: &str,
) -> Result<(), String> {
    if !db_path.exists() {
        return Ok(());
    }
    let mut conn = Connection::open(db_path).map_err(|e| format!("打开 state.vscdb 失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("设置 SQLite busy_timeout 失败: {e}"))?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| format!("开启 state.vscdb 事务失败: {e}"))?;

    let existing_oauth = tx
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1",
            ["antigravityUnifiedStateSync.oauthToken"],
            |row| row.get::<_, String>(0),
        )
        .ok();
    let existing_user = tx
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1",
            ["antigravityUnifiedStateSync.userStatus"],
            |row| row.get::<_, String>(0),
        )
        .ok();

    let next_oauth = patch_oauth_token_topic_b64(existing_oauth.as_deref(), token);
    let next_user = patch_user_status_topic_b64(existing_user.as_deref(), email, name);

    for (k, v) in [
        ("antigravityUnifiedStateSync.oauthToken", next_oauth.as_str()),
        ("antigravityUnifiedStateSync.userStatus", next_user.as_str()),
        ("antigravityOnboarding", "true"),
    ] {
        tx.execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
            [k, v],
        )
        .map_err(|e| format!("写入 state.vscdb 键 {k} 失败: {e}"))?;
    }

    tx.commit().map_err(|e| format!("提交 state.vscdb 事务失败: {e}"))?;
    Ok(())
}

// ---------------- Snapshot / Apply / Rollback / Import ----------------

pub fn capture_snapshot(paths: &CredentialPaths) -> CredentialSnapshot {
    CredentialSnapshot {
        os_cred_raw: read_os_credential_raw(),
        desktop_db_rows: read_state_db_items(&paths.desktop_db),
        ide_db_rows: read_state_db_items(&paths.ide_db),
        google_accounts_raw: fs::read_to_string(&paths.google_accounts).ok(),
        oauth_creds_raw: fs::read_to_string(&paths.oauth_creds).ok(),
        jetski_token_raw: fs::read_to_string(&paths.jetski_token).ok(),
        cli_token_raw: fs::read_to_string(&paths.cli_token).ok(),
    }
}

pub fn apply_credential(paths: &CredentialPaths, cred: &SystemCredentialInput) -> Result<(), String> {
    let expiry_dt = chrono::DateTime::parse_from_rfc3339(&cred.expires_at)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::hours(1));
    let expiry_secs = expiry_dt.timestamp();
    let iso_expiry = expiry_dt.to_rfc3339();

    let mut payload = json!({
        "token": {
            "access_token": cred.access_token,
            "token_type": "Bearer",
            "refresh_token": cred.refresh_token,
            "expiry": iso_expiry
        },
        "auth_method": "consumer",
        "id_token_email": cred.email
    });
    if !cred.id_token.is_empty() {
        payload["id_token"] = json!(cred.id_token);
    }

    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut payload_str = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    // Windows Credential Manager has a 2560-byte blob limit; drop redundant id_token if needed.
    #[cfg(target_os = "windows")]
    if payload_str.len() > 2500 {
        if let Some(obj) = payload.as_object_mut() {
            obj.remove("id_token");
        }
        payload_str = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    }

    // 1. OS shared credential store
    write_os_credential_raw(&payload_str)?;

    // 2. Desktop & IDE state.vscdb Protobuf sync (in-place sentinel patch preserving sibling entries)
    let token_payload = UnifiedOAuthToken {
        access_token: cred.access_token.clone(),
        token_type: "Bearer".into(),
        refresh_token: cred.refresh_token.clone(),
        expiry_seconds: Some(expiry_secs),
        id_token: cred.id_token.clone(),
    };
    patch_state_db_credentials(&paths.desktop_db, &token_payload, &cred.email, &cred.name)?;
    let _ = patch_state_db_credentials(&paths.ide_db, &token_payload, &cred.email, &cred.name);

    // 3. CLI ~/.gemini files
    if let Some(parent) = paths.google_accounts.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut old_list: Vec<String> = fs::read_to_string(&paths.google_accounts)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|v| {
            let mut list: Vec<String> = v
                .get("old")
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(prev) = v.get("active").and_then(|x| x.as_str()) {
                if !prev.is_empty()
                    && !prev.eq_ignore_ascii_case(&cred.email)
                    && !list.iter().any(|x| x.eq_ignore_ascii_case(prev))
                {
                    list.push(prev.to_string());
                }
            }
            list
        })
        .unwrap_or_default();
    old_list.retain(|x| !x.eq_ignore_ascii_case(&cred.email));

    let ga_json = serde_json::to_string_pretty(&json!({
        "active": cred.email,
        "old": old_list
    }))
    .map_err(|e| e.to_string())?;
    fs::write(&paths.google_accounts, ga_json).map_err(|e| format!("写入 google_accounts.json 失败: {e}"))?;

    let mut oauth_creds = json!({
        "access_token": cred.access_token,
        "refresh_token": cred.refresh_token,
        "token_type": "Bearer",
        "expiry_date": expiry_secs * 1000,
        "scope": OAUTH_SCOPES
    });
    if !cred.id_token.is_empty() {
        oauth_creds["id_token"] = json!(cred.id_token);
    }
    fs::write(
        &paths.oauth_creds,
        serde_json::to_string_pretty(&oauth_creds).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写入 oauth_creds.json 失败: {e}"))?;

    let pretty_payload = serde_json::to_string_pretty(&payload).unwrap_or(payload_str);
    if paths.cli_token.parent().map(|p| p.exists()).unwrap_or(false) {
        let _ = fs::write(&paths.cli_token, &pretty_payload);
    }
    if paths.jetski_token.exists() {
        let _ = fs::write(&paths.jetski_token, &pretty_payload);
    }

    Ok(())
}

pub fn restore_snapshot(paths: &CredentialPaths, snap: &CredentialSnapshot) -> Result<(), String> {
    if let Some(raw) = &snap.os_cred_raw {
        write_os_credential_raw(raw)?;
    }
    if !snap.desktop_db_rows.is_empty() {
        let _ = write_state_db_items(&paths.desktop_db, &snap.desktop_db_rows);
    }
    if !snap.ide_db_rows.is_empty() {
        let _ = write_state_db_items(&paths.ide_db, &snap.ide_db_rows);
    }
    restore_file(&paths.google_accounts, snap.google_accounts_raw.as_deref());
    restore_file(&paths.oauth_creds, snap.oauth_creds_raw.as_deref());
    restore_file(&paths.jetski_token, snap.jetski_token_raw.as_deref());
    restore_file(&paths.cli_token, snap.cli_token_raw.as_deref());
    Ok(())
}

fn restore_file(path: &Path, content: Option<&str>) {
    match content {
        Some(c) => {
            if path.parent().map(|p| p.exists()).unwrap_or(false) {
                let _ = fs::write(path, c);
            }
        }
        None => {
            if path.exists() {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// Imports the currently logged-in Antigravity credential from OS Keychain/CredMgr, state.vscdb, or CLI files.
pub fn import_current_credential(paths: &CredentialPaths) -> Result<ImportedCredential, String> {
    // 1. Try OS credential store (Antigravity 2.x primary store)
    if let Some(raw) = read_os_credential_raw() {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            if let Some(parsed) = parse_keyring_json(&v) {
                return Ok(parsed);
            }
        }
    }

    // 2. Try Desktop state.vscdb
    let db_rows = read_state_db_items(&paths.desktop_db);
    if let Some(Some(oauth_b64)) = db_rows.get("antigravityUnifiedStateSync.oauthToken") {
        if let Some(tok) = parse_oauth_token_topic_b64(oauth_b64) {
            let (mut email, mut name) = db_rows
                .get("antigravityUnifiedStateSync.userStatus")
                .and_then(|o| o.as_deref())
                .and_then(parse_user_status_topic_b64)
                .unwrap_or_default();
            if email.is_empty() {
                email = decode_jwt_email(&tok.id_token);
            }
            if name.is_empty() && !email.is_empty() {
                name = email.split('@').next().unwrap_or("").to_string();
            }
            let expires_at = tok
                .expiry_seconds
                .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                .map(|d| d.to_rfc3339())
                .unwrap_or_else(|| (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339());
            return Ok(ImportedCredential {
                email,
                name,
                access_token: tok.access_token,
                refresh_token: tok.refresh_token,
                expires_at,
                id_token: tok.id_token,
            });
        }
    }

    // 3. Try CLI token files
    for p in [&paths.cli_token, &paths.jetski_token] {
        if let Ok(raw) = fs::read_to_string(p) {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                if let Some(parsed) = parse_keyring_json(&v) {
                    return Ok(parsed);
                }
            }
        }
    }

    // 4. Try ~/.gemini/oauth_creds.json + google_accounts.json
    if let Ok(raw) = fs::read_to_string(&paths.oauth_creds) {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            let access = v.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let refresh = v.get("refresh_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if !refresh.is_empty() || !access.is_empty() {
                let id_token = v.get("id_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let mut email = decode_jwt_email(&id_token);
                if email.is_empty() {
                    email = fs::read_to_string(&paths.google_accounts)
                        .ok()
                        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                        .and_then(|ga| ga.get("active").and_then(|x| x.as_str()).map(normalize_email))
                        .unwrap_or_default();
                }
                let expiry_ms = v.get("expiry_date").and_then(|x| x.as_i64()).unwrap_or(0);
                let expires_at = chrono::DateTime::from_timestamp_millis(expiry_ms)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339());
                let name = email.split('@').next().unwrap_or("").to_string();
                return Ok(ImportedCredential {
                    email,
                    name,
                    access_token: access,
                    refresh_token: refresh,
                    expires_at,
                    id_token,
                });
            }
        }
    }

    Err("未在系统凭据库、state.vscdb 或 ~/.gemini 中找到当前已登录的 Antigravity 账号".into())
}

fn parse_keyring_json(v: &Value) -> Option<ImportedCredential> {
    let tok = v.get("token")?;
    let access_token = tok.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let refresh_token = tok.get("refresh_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if access_token.is_empty() && refresh_token.is_empty() {
        return None;
    }
    let id_token = v.get("id_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut email = v
        .get("id_token_email")
        .and_then(|x| x.as_str())
        .map(normalize_email)
        .unwrap_or_default();
    if email.is_empty() {
        email = decode_jwt_email(&id_token);
    }
    let expires_at = tok
        .get("expiry")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339());
    let name = email.split('@').next().unwrap_or("").to_string();
    Some(ImportedCredential {
        email,
        name,
        access_token,
        refresh_token,
        expires_at,
        id_token,
    })
}
