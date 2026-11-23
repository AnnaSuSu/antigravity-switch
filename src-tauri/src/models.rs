use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBucket {
    pub bucket_id: String,
    pub window_key: String,   // "5h" | "24h" | "7d" | "other"
    pub window_label: String, // "5h" | "24h" | "7d" | ...
    pub remaining_fraction: Option<f64>,
    pub reset_time_iso: Option<String>,
    pub reset_at_ms: Option<i64>,
    pub is_exhausted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPool {
    pub pool_key: String,
    pub title: String,
    pub models: Vec<String>,
    pub remaining_fraction: Option<f64>,
    pub is_exhausted: bool,
    pub reset_time_iso: Option<String>,
    pub reset_at_ms: Option<i64>,
    pub buckets: Vec<QuotaBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub email: Option<String>,
    pub plan_name: Option<String>,
    pub pools: Vec<QuotaPool>,
    pub primary_fraction: Option<f64>,
    pub fetched_at_ms: i64,
    pub source: String, // "daemon" | "cloudcode"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub email: String,
    pub name: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
    #[serde(default)]
    pub id_token: String,
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub plan_name: Option<String>,
    #[serde(default)]
    pub last_quota: Option<QuotaSnapshot>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub added_at_ms: i64,
    #[serde(default)]
    pub last_used_at_ms: i64,
}

/// Safe frontend view of an account without exposing raw OAuth secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub email: String,
    pub name: String,
    pub is_active: bool,
    pub plan_name: Option<String>,
    pub last_quota: Option<QuotaSnapshot>,
    pub last_error: Option<String>,
    pub has_refresh_token: bool,
    pub last_used_at_ms: i64,
}

impl AccountRecord {
    pub fn to_summary(&self, active_email: Option<&str>) -> AccountSummary {
        let is_active = active_email
            .map(|a| a.eq_ignore_ascii_case(&self.email))
            .unwrap_or(false);
        AccountSummary {
            email: self.email.clone(),
            name: self.name.clone(),
            is_active,
            plan_name: self.plan_name.clone(),
            last_quota: self.last_quota.clone(),
            last_error: self.last_error.clone(),
            has_refresh_token: !self.refresh_token.is_empty(),
            last_used_at_ms: self.last_used_at_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoSwitchConfig {
    pub enabled: bool,
    pub pool_key: String,   // e.g. "shared_gemini_main" | "shared_3p_pool" | "shared_gemini_argon"
    pub window_key: String, // "primary" | "5h" | "7d"
    pub threshold_pct: u32, // switch when remaining <= threshold_pct
    pub cooldown_sec: u64,
    pub candidates: Vec<String>, // empty = all accounts
}

impl Default for AutoSwitchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            pool_key: "shared_gemini_main".into(),
            window_key: "primary".into(),
            threshold_pct: 10,
            cooldown_sec: 180,
            candidates: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    pub enabled: bool,
    pub proxy_url: String,
    pub bypass_list: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            proxy_url: "http://127.0.0.1:7897".into(),
            bypass_list: "localhost,127.0.0.1,::1,127.0.0.0/8,10.0.0.0/8,192.168.0.0/16,172.16.0.0/12,<local>".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchLog {
    pub at_ms: i64,
    pub from_email: Option<String>,
    pub to_email: String,
    pub trigger: String, // "manual" | "auto"
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayDisplayConfig {
    pub show_quota: bool,
    pub show_context: bool,
    pub show_speed: bool,
}

impl Default for TrayDisplayConfig {
    fn default() -> Self {
        Self {
            show_quota: true,
            show_context: true,
            show_speed: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppStoreData {
    #[serde(default)]
    pub active_email: Option<String>,
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountRecord>,
    #[serde(default)]
    pub auto_switch: AutoSwitchConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub tray_display: TrayDisplayConfig,
    #[serde(default)]
    pub switch_restarts_app: bool,
    #[serde(default)]
    pub last_switch: Option<SwitchLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListItem {
    pub cascade_id: String,
    pub title: String,
    pub modified_at_ms: i64,
    pub source: String, // "desktop" | "cli"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSpeedMetric {
    pub model: String,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub ttft_sec: Option<f64>,
    pub streaming_sec: Option<f64>,
    pub total_sec: Option<f64>,
    pub tps: Option<f64>,
    pub streaming_tps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextSnapshot {
    pub cascade_id: String,
    pub model: String,
    pub used_tokens: Option<u64>,
    pub budget_tokens: Option<u64>,
    pub percent: Option<f64>,
    pub input_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub thinking_tokens: Option<u64>,
    pub estimated: bool,
    pub latest_speed: Option<TurnSpeedMetric>,
    pub recent_turns: Vec<TurnSpeedMetric>,
    pub daemon_connected: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
    pub thinking: u64,
    pub total: u64,
    pub requests: u64,
    pub cache_hit_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsageRow {
    pub date: String, // YYYY-MM-DD
    pub desktop_total: u64,
    pub cli_total: u64,
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
    pub total: u64,
    pub requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageRow {
    pub model: String,
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
    pub total: u64,
    pub requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub today: TokenTotals,
    pub last_7d: TokenTotals,
    pub last_30d: TokenTotals,
    pub all_time: TokenTotals,
    pub desktop_total: u64,
    pub cli_total: u64,
    pub daily: Vec<DailyUsageRow>,
    pub by_model: Vec<ModelUsageRow>,
    pub scanned_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub status_code: Option<u16>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchResult {
    pub ok: bool,
    pub email: String,
    pub verified: bool,
    pub rolled_back: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardOverview {
    pub daemon_online: bool,
    pub daemon_email: Option<String>,
    pub active_email: Option<String>,
    pub accounts: Vec<AccountSummary>,
    pub active_quota: Option<QuotaSnapshot>,
    pub auto_switch: AutoSwitchConfig,
    pub proxy: ProxyConfig,
    pub tray_display: TrayDisplayConfig,
    pub switch_restarts_app: bool,
    pub antigravity_app_path: Option<String>,
    pub last_switch: Option<SwitchLog>,
}
