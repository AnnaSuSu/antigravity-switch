use crate::models::{ProxyConfig, ProxyTestResult};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

pub fn normalize_proxy_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

pub fn validate_proxy_url(raw: &str) -> Result<String, String> {
    let norm = normalize_proxy_url(raw);
    if norm.is_empty() {
        return Err("代理地址不能为空（如 http://127.0.0.1:7897）".into());
    }
    let parsed = url::Url::parse(&norm).map_err(|_| "代理地址格式无效".to_string())?;
    match parsed.scheme() {
        "http" | "https" | "socks5" | "socks5h" => {}
        s => return Err(format!("不支持的代理协议: {s}（支持 http/https/socks5）")),
    }
    if parsed.host_str().unwrap_or("").is_empty() {
        return Err("代理地址缺少主机名".into());
    }
    Ok(norm)
}

/// Builds an HTTP client honoring the configured proxy (or direct if disabled).
pub fn build_external_client(proxy: Option<&ProxyConfig>, timeout_secs: u64) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .connect_timeout(Duration::from_secs(timeout_secs.min(10)));

    if let Some(cfg) = proxy.filter(|p| p.enabled && !p.proxy_url.trim().is_empty()) {
        let url = validate_proxy_url(&cfg.proxy_url)?;
        let p = reqwest::Proxy::all(&url).map_err(|e| format!("代理配置无效: {e}"))?;
        builder = builder.proxy(p);
    }

    builder.build().map_err(|e| format!("初始化 HTTP 客户端失败: {e}"))
}

/// Builds a loopback-only HTTPS client for local Antigravity LanguageServer RPCs.
pub fn build_loopback_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .map_err(|e| format!("初始化本地 RPC 客户端失败: {e}"))
}

pub async fn test_proxy_connection(proxy_url: &str) -> ProxyTestResult {
    let validated = match validate_proxy_url(proxy_url) {
        Ok(u) => u,
        Err(e) => {
            return ProxyTestResult {
                ok: false,
                latency_ms: None,
                status_code: None,
                message: e,
            };
        }
    };

    let cfg = ProxyConfig {
        enabled: true,
        proxy_url: validated.clone(),
        bypass_list: String::new(),
    };
    let client = match build_external_client(Some(&cfg), 8) {
        Ok(c) => c,
        Err(e) => {
            return ProxyTestResult {
                ok: false,
                latency_ms: None,
                status_code: None,
                message: e,
            };
        }
    };

    let start = Instant::now();
    match client.get("https://oauth2.googleapis.com/").send().await {
        Ok(resp) => {
            let ms = start.elapsed().as_millis() as u64;
            let status = resp.status().as_u16();
            // Any HTTP response from Google's endpoint confirms proxy tunnel + TLS reachability.
            ProxyTestResult {
                ok: true,
                latency_ms: Some(ms),
                status_code: Some(status),
                message: format!("连通正常 ({ms} ms, HTTP {status})"),
            }
        }
        Err(e) => ProxyTestResult {
            ok: false,
            latency_ms: None,
            status_code: None,
            message: format!("代理连接失败: {e}"),
        },
    }
}

/// Locates the Antigravity Desktop executable/bundle on macOS or Windows.
pub fn detect_antigravity_app_path() -> Option<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut candidates: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/Applications/Antigravity.app"));
        candidates.push(home.join("Applications/Antigravity.app"));
        candidates.push(PathBuf::from("/Applications/Google Antigravity.app"));
        candidates.push(home.join("Applications/Google Antigravity.app"));
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            candidates.push(PathBuf::from(&local).join("Programs").join("antigravity").join("Antigravity.exe"));
            candidates.push(PathBuf::from(&local).join("Programs").join("Antigravity").join("Antigravity.exe"));
        }
        if let Ok(pf) = std::env::var("ProgramFiles") {
            candidates.push(PathBuf::from(&pf).join("antigravity").join("Antigravity.exe"));
            candidates.push(PathBuf::from(&pf).join("Google Antigravity").join("Antigravity.exe"));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        candidates.push(PathBuf::from("/opt/antigravity/antigravity"));
        candidates.push(PathBuf::from("/usr/bin/antigravity"));
        let _ = home;
    }

    candidates.into_iter().find(|p| p.exists())
}

/// Launches (or restarts) Antigravity Desktop with No-TUN proxy env vars and Chromium flags.
pub fn launch_antigravity_with_proxy(proxy: &ProxyConfig, restart_if_running: bool) -> Result<String, String> {
    let app_path = detect_antigravity_app_path()
        .ok_or_else(|| "未检测到 Antigravity 客户端安装路径".to_string())?;

    let proxy_url = if proxy.enabled {
        Some(validate_proxy_url(&proxy.proxy_url)?)
    } else {
        None
    };

    if restart_if_running {
        terminate_antigravity_desktop();
        std::thread::sleep(Duration::from_millis(900));
    }

    #[cfg(target_os = "macos")]
    {
        let bin_path = app_path.join("Contents/MacOS/Antigravity");
        let mut cmd = if bin_path.exists() {
            Command::new(&bin_path)
        } else {
            let mut open_cmd = Command::new("/usr/bin/open");
            open_cmd.arg("-na").arg(&app_path).arg("--args");
            open_cmd
        };
        apply_proxy_env_and_args(&mut cmd, proxy_url.as_deref(), &proxy.bypass_list);
        cmd.spawn().map_err(|e| format!("启动 Antigravity 失败: {e}"))?;
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let mut cmd = Command::new(&app_path);
        apply_proxy_env_and_args(&mut cmd, proxy_url.as_deref(), &proxy.bypass_list);
        cmd.creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("启动 Antigravity 失败: {e}"))?;
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut cmd = Command::new(&app_path);
        apply_proxy_env_and_args(&mut cmd, proxy_url.as_deref(), &proxy.bypass_list);
        cmd.spawn().map_err(|e| format!("启动 Antigravity 失败: {e}"))?;
    }

    match proxy_url {
        Some(u) => Ok(format!("已通过代理 {u} 启动 Antigravity")),
        None => Ok("已直连启动 Antigravity".into()),
    }
}

fn apply_proxy_env_and_args(cmd: &mut Command, proxy_url: Option<&str>, bypass_list: &str) {
    if let Some(url) = proxy_url {
        let chromium_proxy = url.replacen("socks5h://", "socks5://", 1);
        for key in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
            cmd.env(key, url);
        }
        cmd.env("NODE_USE_ENV_PROXY", "1");
        if !bypass_list.trim().is_empty() {
            cmd.env("NO_PROXY", bypass_list.trim());
            cmd.env("no_proxy", bypass_list.trim());
        }
        cmd.arg(format!("--proxy-server={chromium_proxy}"));
        if !bypass_list.trim().is_empty() {
            cmd.arg(format!("--proxy-bypass-list={}", bypass_list.trim()));
        }
    } else {
        for key in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"] {
            cmd.env_remove(key);
        }
    }
}

fn terminate_antigravity_desktop() {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("/usr/bin/osascript")
            .args(["-e", "tell application \"Antigravity\" to quit"])
            .output();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill.exe")
            .args(["/IM", "Antigravity.exe", "/T", "/F"])
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_supported_proxy_urls() {
        assert_eq!(
            validate_proxy_url("  http://127.0.0.1:7897/ ").unwrap(),
            "http://127.0.0.1:7897"
        );
        assert_eq!(
            validate_proxy_url("socks5h://127.0.0.1:1080").unwrap(),
            "socks5h://127.0.0.1:1080"
        );
        assert!(validate_proxy_url("ftp://127.0.0.1:21").is_err());
        assert!(validate_proxy_url("").is_err());
    }
}

