use crate::models::ProxyConfig;
use crate::proxy::build_external_client;
use crate::store::normalize_email;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const OAUTH_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/auth";
const OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const OAUTH_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
const OAUTH_SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform \
https://www.googleapis.com/auth/userinfo.email \
https://www.googleapis.com/auth/userinfo.profile \
https://www.googleapis.com/auth/cclog \
https://www.googleapis.com/auth/experimentsandconfigs";

/// SHA-256 anchors for official Antigravity OAuth client pairs; secrets are read from the user's
/// local `language_server` binary at runtime and never hardcoded in source.
const OFFICIAL_CLIENT_ANCHORS: &[(&str, &str)] = &[
    (
        "bf00c418024ba6bf606ccdc37120976e41bc429dd1d46ecf16a729aa532626ea",
        "1d2f041093fd95aa8995a038c711d50a7960da09a505381c09a745d6ad0ecc60",
    ),
    (
        "69dee430c4e956f6b672b99a846566d6a86814999fc3b3b050697c0d65ea80c3",
        "a2ddedc84850a9c48b7e01d61700848083bf0e10fb8d6c99c496803e8b48ced0",
    ),
];

#[derive(Debug, Clone)]
pub struct OAuthClientConfig {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone)]
pub struct TokenBundle {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
    pub id_token: String,
    pub client_id: String,
}

#[derive(Debug, Clone)]
pub struct GoogleUserInfo {
    pub email: String,
    pub name: String,
}

static HOST_OAUTH_CACHE: Mutex<Option<Vec<OAuthClientConfig>>> = Mutex::new(None);
static OAUTH_CANCEL_TX: Mutex<Option<oneshot::Sender<()>>> = Mutex::new(None);

/// Cancels an in-flight loopback OAuth login so the local listener shuts down immediately.
pub fn cancel_oauth_login() -> bool {
    if let Ok(mut guard) = OAUTH_CANCEL_TX.lock() {
        if let Some(tx) = guard.take() {
            let _ = tx.send(());
            return true;
        }
    }
    false
}

fn detect_running_language_server_paths() -> Vec<PathBuf> {
    let mut found = Vec::new();
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("/bin/ps")
            .args(["-ax", "-o", "comm="])
            .output()
        {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let p = line.trim();
                if (p.contains("Antigravity") || p.contains("antigravity"))
                    && p.contains("language_server")
                {
                    let pb = PathBuf::from(p);
                    if pb.exists() && !found.contains(&pb) {
                        found.push(pb);
                    }
                }
            }
        }
    }
    found
}

pub fn detect_language_server_binaries() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut candidates = detect_running_language_server_paths();

    #[cfg(target_os = "macos")]
    {
        let static_paths = [
            PathBuf::from("/Applications/Antigravity.app/Contents/Resources/bin/language_server"),
            home.join("Applications/Antigravity.app/Contents/Resources/bin/language_server"),
            PathBuf::from("/Applications/Antigravity.app/Contents/Resources/app/extensions/antigravity/bin/language_server_darwin_arm64"),
            PathBuf::from("/Applications/Antigravity.app/Contents/Resources/app/extensions/antigravity/bin/language_server_darwin_x64"),
            home.join("Applications/Antigravity.app/Contents/Resources/app/extensions/antigravity/bin/language_server_darwin_arm64"),
            PathBuf::from("/Applications/Antigravity IDE.app/Contents/Resources/bin/language_server"),
            home.join("Applications/Antigravity IDE.app/Contents/Resources/bin/language_server"),
            PathBuf::from("/Applications/Google Antigravity.app/Contents/Resources/bin/language_server"),
            home.join("Applications/Google Antigravity.app/Contents/Resources/bin/language_server"),
        ];
        for p in static_paths {
            if p.exists() && !candidates.contains(&p) {
                candidates.push(p);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let mut static_paths = Vec::new();
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            static_paths.push(PathBuf::from(&local).join("Programs/antigravity/resources/bin/language_server.exe"));
            static_paths.push(PathBuf::from(&local).join("Programs/Antigravity/resources/bin/language_server.exe"));
            static_paths.push(PathBuf::from(&local).join("Programs/antigravity/resources/app/extensions/antigravity/bin/language_server_windows_x64.exe"));
        }
        if let Ok(pf) = std::env::var("ProgramFiles") {
            static_paths.push(PathBuf::from(&pf).join("antigravity/resources/bin/language_server.exe"));
            static_paths.push(PathBuf::from(&pf).join("Google Antigravity/resources/bin/language_server.exe"));
        }
        for p in static_paths {
            if p.exists() && !candidates.contains(&p) {
                candidates.push(p);
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        for p in [
            PathBuf::from("/opt/antigravity/resources/bin/language_server"),
            home.join(".local/share/antigravity/resources/bin/language_server"),
        ] {
            if p.exists() && !candidates.contains(&p) {
                candidates.push(p);
            }
        }
    }

    candidates
}

fn sha256_hex(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Extracts Google OAuth client IDs (`[0-9]{10,}-[a-z0-9_-]+.apps.googleusercontent.com`)
/// from Go `.rodata` slices where adjacent string literals are packed without NUL separators.
fn match_client_ids_in_slice(
    work: &[u8],
    wanted_ids: &HashSet<&str>,
    id_by_hash: &mut HashMap<String, String>,
) {
    let id_suffix = b".apps.googleusercontent.com";
    let mut pos = 0;
    while id_by_hash.len() < wanted_ids.len() {
        let Some(rel) = find_subslice(&work[pos..], id_suffix) else {
            break;
        };
        let idx = pos + rel;
        let end = idx + id_suffix.len();
        let win_start = idx.saturating_sub(64);

        // Walk backward through valid client-id body chars, checking every `<digits>-<body_suffix>` split.
        let mut cursor = idx;
        while cursor > win_start {
            let b = work[cursor - 1];
            if !(b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_') {
                break;
            }
            cursor -= 1;
            if b == b'-' && cursor + 1 < idx {
                let mut d_start = cursor;
                while d_start > win_start && work[d_start - 1].is_ascii_digit() {
                    d_start -= 1;
                }
                if cursor - d_start >= 10 {
                    // Slide start across leading digits in case the adjacent Go literal ended in a digit.
                    for s in d_start..=(cursor - 10) {
                        if let Ok(cand) = std::str::from_utf8(&work[s..end]) {
                            let h = sha256_hex(cand);
                            if wanted_ids.contains(h.as_str()) {
                                id_by_hash.entry(h).or_insert_with(|| cand.to_string());
                                break;
                            }
                        }
                    }
                }
            }
        }
        pos = end;
    }
}

fn match_client_secrets_in_slice(
    work: &[u8],
    wanted_secs: &HashSet<&str>,
    sec_by_hash: &mut HashMap<String, String>,
) {
    let sec_prefix = b"GOCSPX-";
    let mut pos = 0;
    while sec_by_hash.len() < wanted_secs.len() {
        let Some(rel) = find_subslice(&work[pos..], sec_prefix) else {
            break;
        };
        let idx = pos + rel;
        if idx + 35 <= work.len() {
            if let Ok(cand) = std::str::from_utf8(&work[idx..idx + 35]) {
                if cand.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                    let h = sha256_hex(cand);
                    if wanted_secs.contains(h.as_str()) {
                        sec_by_hash.entry(h).or_insert_with(|| cand.to_string());
                    }
                }
            }
        }
        pos = idx + sec_prefix.len();
    }
}

fn scan_binary_oauth_pairs(bin_path: &Path) -> Vec<OAuthClientConfig> {
    let Ok(mut file) = File::open(bin_path) else {
        return Vec::new();
    };
    let wanted_ids: HashSet<&str> = OFFICIAL_CLIENT_ANCHORS.iter().map(|a| a.0).collect();
    let wanted_secs: HashSet<&str> = OFFICIAL_CLIENT_ANCHORS.iter().map(|a| a.1).collect();
    let mut id_by_hash: HashMap<String, String> = HashMap::new();
    let mut sec_by_hash: HashMap<String, String> = HashMap::new();

    const CHUNK_SIZE: usize = 4 * 1024 * 1024;
    const OVERLAP: usize = 512;

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut tail = Vec::new();

    while id_by_hash.len() < wanted_ids.len() || sec_by_hash.len() < wanted_secs.len() {
        let Ok(n) = file.read(&mut buf) else {
            break;
        };
        if n == 0 {
            break;
        }
        let mut work = Vec::with_capacity(tail.len() + n);
        work.extend_from_slice(&tail);
        work.extend_from_slice(&buf[..n]);

        if id_by_hash.len() < wanted_ids.len() {
            match_client_ids_in_slice(&work, &wanted_ids, &mut id_by_hash);
        }
        if sec_by_hash.len() < wanted_secs.len() {
            match_client_secrets_in_slice(&work, &wanted_secs, &mut sec_by_hash);
        }

        tail = if work.len() > OVERLAP {
            work[work.len() - OVERLAP..].to_vec()
        } else {
            work
        };
    }

    let mut pairs = Vec::new();
    for (id_h, sec_h) in OFFICIAL_CLIENT_ANCHORS {
        if let (Some(cid), Some(csec)) = (id_by_hash.get(*id_h), sec_by_hash.get(*sec_h)) {
            pairs.push(OAuthClientConfig {
                client_id: cid.clone(),
                client_secret: csec.clone(),
            });
        }
    }
    pairs
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = 0;
    let limit = haystack.len() - needle.len();
    while i <= limit {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub fn resolve_oauth_configs(preferred_client_id: Option<&str>) -> Vec<OAuthClientConfig> {
    let env_id = std::env::var("ANTIGRAVITY_OAUTH_CLIENT_ID")
        .or_else(|_| std::env::var("AG_OAUTH_CLIENT_ID"))
        .unwrap_or_default();
    let env_sec = std::env::var("ANTIGRAVITY_OAUTH_CLIENT_SECRET")
        .or_else(|_| std::env::var("AG_OAUTH_CLIENT_SECRET"))
        .unwrap_or_default();
    if !env_id.trim().is_empty() && !env_sec.trim().is_empty() {
        return vec![OAuthClientConfig {
            client_id: env_id.trim().to_string(),
            client_secret: env_sec.trim().to_string(),
        }];
    }

    let mut list = {
        let guard = HOST_OAUTH_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone().unwrap_or_default()
    };

    if list.is_empty() {
        for bin in detect_language_server_binaries() {
            let scanned = scan_binary_oauth_pairs(&bin);
            if !scanned.is_empty() {
                list = scanned;
                if let Ok(mut guard) = HOST_OAUTH_CACHE.lock() {
                    *guard = Some(list.clone());
                }
                break;
            }
        }
    }

    if let Some(pref) = preferred_client_id.filter(|s| !s.is_empty()) {
        list.sort_by_key(|c| if c.client_id == pref { 0 } else { 1 });
    }
    list
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Starts a one-shot loopback OAuth2 flow, opens browser, waits for callback, and returns tokens + user info.
pub async fn run_oauth_browser_login(
    proxy: Option<&ProxyConfig>,
) -> Result<(TokenBundle, GoogleUserInfo), String> {
    let configs = resolve_oauth_configs(None);
    let cfg = configs.first().ok_or_else(|| {
        let bins = detect_language_server_binaries();
        if bins.is_empty() {
            "未检测到本机 Antigravity language_server 二进制，请先安装 Antigravity 或点击「导入当前」".to_string()
        } else {
            format!(
                "已检测到 {} 但未匹配到 OAuth 配置（可能客户端版本已变更），可先使用「导入当前」或设置 AG_OAUTH_CLIENT_ID / AG_OAUTH_CLIENT_SECRET",
                bins[0].display()
            )
        }
    })?;

    let mut listener = None;
    let mut port = 0u16;
    for p in 11451..11531 {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", p)).await {
            port = p;
            listener = Some(l);
            break;
        }
    }
    let listener = listener.ok_or_else(|| "无法绑定本地 OAuth 回调端口".to_string())?;
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth-callback");
    let state = format!("agsw_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));

    let mut auth_url = url::Url::parse(OAUTH_AUTH_URL).map_err(|e| e.to_string())?;
    auth_url
        .query_pairs_mut()
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", OAUTH_SCOPES)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", &state);

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    if let Ok(mut guard) = OAUTH_CANCEL_TX.lock() {
        if let Some(prev) = guard.replace(cancel_tx) {
            let _ = prev.send(());
        }
    }

    open::that(auth_url.as_str()).map_err(|e| {
        cancel_oauth_login();
        format!("打开浏览器失败: {e}")
    })?;

    let code_res = tokio::select! {
        res = tokio::time::timeout(
            std::time::Duration::from_secs(180),
            wait_for_oauth_code(listener, &state),
        ) => {
            res.map_err(|_| "等待浏览器 OAuth 登录超时（3分钟）".to_string())?
        }
        _ = cancel_rx => {
            Err("已取消登录授权".to_string())
        }
    };

    if let Ok(mut guard) = OAUTH_CANCEL_TX.lock() {
        let _ = guard.take();
    }

    let code = code_res?;
    let tokens = exchange_code(&code, &redirect_uri, cfg, proxy).await?;
    let user = fetch_userinfo(&tokens.access_token, proxy).await?;
    Ok((tokens, user))
}

async fn wait_for_oauth_code(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    loop {
        let (mut socket, _) = listener.accept().await.map_err(|e| format!("回调连接失败: {e}"))?;
        let mut buf = vec![0u8; 4096];
        let n = socket.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            continue;
        }
        let req = String::from_utf8_lossy(&buf[..n]);
        let first_line = req.lines().next().unwrap_or("");
        let path_part = first_line.split_whitespace().nth(1).unwrap_or("/");
        if !path_part.starts_with("/oauth-callback") {
            let _ = socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            continue;
        }
        let full_url = format!("http://127.0.0.1{path_part}");
        let parsed = url::Url::parse(&full_url).map_err(|e| e.to_string())?;
        let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();

        if let Some(err) = params.get("error") {
            let html = format!("<html><body style='font-family:sans-serif;padding:40px'><h3>授权取消或失败</h3><p>{err}</p></body></html>");
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", html.len(), html);
            let _ = socket.write_all(resp.as_bytes()).await;
            return Err(format!("Google 授权失败: {err}"));
        }

        if params.get("state").map(|s| s.as_str()) != Some(expected_state) {
            return Err("OAuth state 校验失败".into());
        }

        if let Some(code) = params.get("code") {
            let html = "<html><body style='font-family:sans-serif;padding:40px;text-align:center'><h2>登录成功</h2><p>可以关闭此页面并返回 Antigravity Switch。</p></body></html>";
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", html.len(), html);
            let _ = socket.write_all(resp.as_bytes()).await;
            return Ok(code.clone());
        }
    }
}

async fn exchange_code(
    code: &str,
    redirect_uri: &str,
    cfg: &OAuthClientConfig,
    proxy: Option<&ProxyConfig>,
) -> Result<TokenBundle, String> {
    let client = build_external_client(proxy, 20)?;
    let resp = client
        .post(OAUTH_TOKEN_URL)
        .form(&[
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| format!("换取 Token 网络失败: {e}"))?;

    let status = resp.status();
    let body: TokenResponse = resp.json().await.map_err(|e| format!("解析 Token 响应失败: {e}"))?;
    let Some(access_token) = body.access_token else {
        let err = body.error_description.or(body.error).unwrap_or_else(|| status.to_string());
        return Err(format!("OAuth 换取 Token 失败: {err}"));
    };
    let expires_in = body.expires_in.unwrap_or(3600);
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(expires_in)).to_rfc3339();
    Ok(TokenBundle {
        access_token,
        refresh_token: body.refresh_token.unwrap_or_default(),
        expires_at,
        id_token: body.id_token.unwrap_or_default(),
        client_id: cfg.client_id.clone(),
    })
}

pub async fn refresh_access_token(
    refresh_token: &str,
    preferred_client_id: Option<&str>,
    proxy: Option<&ProxyConfig>,
) -> Result<TokenBundle, String> {
    if refresh_token.trim().is_empty() {
        return Err("缺少 refresh_token，请重新登录该账号".into());
    }
    let configs = resolve_oauth_configs(preferred_client_id);
    if configs.is_empty() {
        return Err("未检测到本机 OAuth 客户端配置，无法刷新 Token".into());
    }
    let client = build_external_client(proxy, 15)?;
    let mut last_err = String::new();

    for cfg in &configs {
        let resp = client
            .post(OAUTH_TOKEN_URL)
            .form(&[
                ("client_id", cfg.client_id.as_str()),
                ("client_secret", cfg.client_secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| format!("刷新 Token 网络失败: {e}"))?;

        let status = resp.status();
        let body: TokenResponse = resp.json().await.map_err(|e| format!("解析刷新响应失败: {e}"))?;
        if let Some(access_token) = body.access_token {
            let expires_in = body.expires_in.unwrap_or(3600);
            let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(expires_in)).to_rfc3339();
            return Ok(TokenBundle {
                access_token,
                refresh_token: body.refresh_token.unwrap_or_else(|| refresh_token.to_string()),
                expires_at,
                id_token: body.id_token.unwrap_or_default(),
                client_id: cfg.client_id.clone(),
            });
        }

        let err_code = body.error.unwrap_or_default();
        let err_desc = body.error_description.unwrap_or_default();
        last_err = format!("刷新 Token 失败 (HTTP {status}): {err_code} {err_desc}").trim().to_string();
        // Retry with alternate official client pair only on client mismatch.
        if err_code != "invalid_client" && err_code != "unauthorized_client" {
            break;
        }
    }

    Err(last_err)
}

pub async fn fetch_userinfo(
    access_token: &str,
    proxy: Option<&ProxyConfig>,
) -> Result<GoogleUserInfo, String> {
    let client = build_external_client(proxy, 15)?;
    let resp = client
        .get(OAUTH_USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("获取用户信息失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("获取 Google 用户信息失败 (HTTP {})", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let email = v
        .get("email")
        .and_then(|x| x.as_str())
        .map(normalize_email)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Google 用户信息中缺少邮箱".to_string())?;
    let name = v
        .get("name")
        .or_else(|| v.get("given_name"))
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| email.split('@').next().unwrap_or("User").to_string());
    Ok(GoogleUserInfo { email, name })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_go_rodata_packed_oauth_client_id_and_secret() {
        let fake_id = "123456789012-abc9_def-xyz.apps.googleusercontent.com";
        let fake_sec = "GOCSPX-AbCdEfGhIjKlMnOpQrStUvWxYz12";
        assert_eq!(fake_sec.len(), 35);

        let id_hash = sha256_hex(fake_id);
        let sec_hash = sha256_hex(fake_sec);
        let wanted_ids: HashSet<&str> = [id_hash.as_str()].into_iter().collect();
        let wanted_secs: HashSet<&str> = [sec_hash.as_str()].into_iter().collect();

        // Simulate Go .rodata where preceding string ends with a digit and no NUL delimiter exists.
        let packed = format!("precedingGoString99{fake_id}followingString{fake_sec}tail");
        let mut ids = HashMap::new();
        let mut secs = HashMap::new();
        match_client_ids_in_slice(packed.as_bytes(), &wanted_ids, &mut ids);
        match_client_secrets_in_slice(packed.as_bytes(), &wanted_secs, &mut secs);

        assert_eq!(ids.get(&id_hash).map(String::as_str), Some(fake_id));
        assert_eq!(secs.get(&sec_hash).map(String::as_str), Some(fake_sec));
    }
}

