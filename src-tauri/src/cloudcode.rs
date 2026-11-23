use crate::models::{ProxyConfig, QuotaBucket, QuotaPool, QuotaSnapshot};
use crate::proxy::{build_external_client, detect_antigravity_app_path};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;

const CLOUDCODE_PROD_URL: &str = "https://cloudcode-pa.googleapis.com";
static ANTIGRAVITY_VERSION_CACHE: OnceLock<String> = OnceLock::new();

/// Reads the installed Antigravity Desktop version so CloudCode User-Agent stays aligned with host updates.
fn detect_antigravity_version() -> &'static str {
    ANTIGRAVITY_VERSION_CACHE.get_or_init(|| {
        if let Some(app_path) = detect_antigravity_app_path() {
            // macOS bundle: Contents/Info.plist -> CFBundleShortVersionString
            #[cfg(target_os = "macos")]
            {
                let plist = app_path.join("Contents/Info.plist");
                if let Ok(text) = std::fs::read_to_string(&plist) {
                    if let Some(ver) = extract_plist_string(&text, "CFBundleShortVersionString") {
                        return ver;
                    }
                }
            }
            // Cross-platform fallback: resources/app/package.json
            let pkg_candidates = [
                app_path.join("Contents/Resources/app/package.json"),
                app_path
                    .parent()
                    .map(|p| p.join("resources/app/package.json"))
                    .unwrap_or_default(),
            ];
            for pkg in pkg_candidates {
                if let Ok(raw) = std::fs::read_to_string(&pkg) {
                    if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                        if let Some(ver) = v.get("version").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
                            return ver.to_string();
                        }
                    }
                }
            }
        }
        "2.32.1".to_string()
    })
}

#[cfg(target_os = "macos")]
fn extract_plist_string(xml: &str, key: &str) -> Option<String> {
    let needle = format!("<key>{key}</key>");
    let start = xml.find(&needle)? + needle.len();
    let rest = &xml[start..];
    let s_open = rest.find("<string>")? + "<string>".len();
    let s_close = rest[s_open..].find("</string>")?;
    let val = rest[s_open..s_open + s_close].trim();
    (!val.is_empty()).then(|| val.to_string())
}

pub fn shared_pool_key(raw: &str) -> String {
    let s = raw.to_lowercase();
    if s.contains("3p") || s.contains("claude") || s.contains("opus") || s.contains("sonnet") || s.contains("gpt") {
        "shared_3p_pool".into()
    } else if s.contains("argon") || s.contains("m668") || s.contains("orchestrator") {
        "shared_gemini_argon".into()
    } else if s.contains("carbon") || s.contains("m670") {
        "shared_gemini_carbon".into()
    } else if s.contains("hydrogen") || s.contains("flash") || s.contains("gemini") {
        "shared_gemini_main".into()
    } else {
        "shared_other".into()
    }
}

pub fn default_pool_title(pool_key: &str) -> &'static str {
    match pool_key {
        "shared_gemini_main" => "Gemini 4 Hydrogen / Flash",
        "shared_3p_pool" => "Claude 5.5 / GPT-OSS (3P)",
        "shared_gemini_argon" => "Gemini 4 Argon / Orchestrator",
        "shared_gemini_carbon" => "Gemini 4 Carbon",
        _ => "Other Models",
    }
}

fn classify_window(window_str: &str, bucket_id: &str) -> (String, String) {
    let w = format!("{} {}", window_str, bucket_id).to_lowercase();
    if w.contains("5h") || w.contains("5_hour") || w.contains("hour") {
        ("5h".into(), "5h".into())
    } else if w.contains("weekly") || w.contains("7d") || w.contains("week") {
        ("7d".into(), "7d".into())
    } else if w.contains("daily") || w.contains("24h") || w.contains("day") {
        ("24h".into(), "24h".into())
    } else {
        ("other".into(), "周期".into())
    }
}

fn window_rank(window_key: &str) -> u8 {
    match window_key {
        "5h" => 0,
        "24h" => 1,
        "7d" => 2,
        _ => 10,
    }
}

fn parse_reset_ms(iso: Option<&str>) -> Option<i64> {
    let s = iso?.trim();
    if s.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

fn parse_fraction(obj: &Value, allow_proto3_zero: bool) -> (Option<f64>, bool) {
    for k in ["remainingFraction", "remaining_fraction", "fraction"] {
        if let Some(f) = obj.get(k).and_then(|v| v.as_f64()) {
            let clamped = f.clamp(0.0, 1.0);
            return (Some(clamped), clamped <= 0.0);
        }
    }
    for k in ["remainingAmount", "remaining_amount", "remaining"] {
        let amt = obj.get(k).and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        });
        if let Some(a) = amt {
            if a <= 0.0 {
                return (Some(0.0), true);
            }
            let total = obj
                .get("limit")
                .or_else(|| obj.get("totalAmount"))
                .or_else(|| obj.get("total"))
                .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
            if let Some(t) = total.filter(|&t| t > 0.0) {
                let clamped = (a / t).clamp(0.0, 1.0);
                return (Some(clamped), clamped <= 0.0);
            }
        }
    }
    // Proto3 omits float 0.0 on QuotaInfo; treat as exhausted when resetTime is present.
    if allow_proto3_zero {
        let disabled = obj.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let has_reset = obj
            .get("resetTime")
            .or_else(|| obj.get("reset_time"))
            .and_then(|v| v.as_str())
            .and_then(|s| parse_reset_ms(Some(s)))
            .is_some();
        if !disabled && has_reset {
            return (Some(0.0), true);
        }
    }
    (None, false)
}

/// Resolves the subscription tier badge, prioritizing authoritative `userTier` and `teamsTier`
/// over legacy `planInfo.planName` (which Antigravity often reports as `"Pro"` even for Ultra/Free).
pub fn resolve_plan_badge(user_root: Option<&Value>, fallback_plan: Option<&str>) -> Option<String> {
    let user_tier_name = user_root
        .and_then(|u| u.pointer("/userTier/name"))
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let user_tier_id = user_root
        .and_then(|u| u.pointer("/userTier/id"))
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let teams_tier = user_root
        .and_then(|u| u.pointer("/planStatus/planInfo/teamsTier"))
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let legacy_plan = user_root
        .and_then(|u| {
            u.pointer("/planStatus/planInfo/planName")
                .or_else(|| u.pointer("/planStatus/planName"))
                .and_then(|x| x.as_str())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let fb = fallback_plan.map(str::trim).filter(|s| !s.is_empty());

    // Authoritative tier sources first; fall back to legacy planName only when absent.
    let auth_signals: Vec<String> = [user_tier_name, user_tier_id, teams_tier, fb]
        .into_iter()
        .flatten()
        .map(|s| s.to_lowercase())
        .collect();

    let signals = if !auth_signals.is_empty() {
        auth_signals
    } else if let Some(lp) = legacy_plan {
        vec![lp.to_lowercase()]
    } else {
        return None;
    };

    if signals.iter().any(|s| s.contains("ultra") || s.contains("pro_ultimate")) {
        return Some("Ultra".into());
    }
    if signals.iter().any(|s| s.contains("enterprise")) {
        return Some("Enterprise".into());
    }
    if signals.iter().any(|s| s.contains("plus")) {
        return Some("Plus".into());
    }
    if signals.iter().any(|s| s.contains("free") || s.contains("standard-tier")) {
        return Some("Free".into());
    }
    if signals.iter().any(|s| s.contains("pro")) {
        return Some("Pro".into());
    }

    user_tier_name
        .or(teams_tier)
        .or(fb)
        .or(legacy_plan)
        .map(str::to_string)
}

/// Normalizes either local daemon RPC responses or remote CloudCode responses into one QuotaSnapshot.
pub fn parse_quota_snapshot(
    user_status_res: Option<&Value>,
    quota_summary_res: Option<&Value>,
    fallback_email: Option<&str>,
    fallback_plan: Option<&str>,
    source: &str,
) -> QuotaSnapshot {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let user_root = user_status_res.and_then(|v| v.get("userStatus").or(Some(v)));

    let email = user_root
        .and_then(|u| u.get("email").and_then(|x| x.as_str()))
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .or_else(|| fallback_email.map(|s| s.trim().to_lowercase()));

    let plan_name = resolve_plan_badge(user_root, fallback_plan);

    let mut pools: BTreeMap<String, QuotaPool> = BTreeMap::new();

    // 1. Parse authoritative quota summary groups (5h / 7d buckets)
    let summary_root = quota_summary_res.and_then(|v| v.get("response").or(Some(v)));
    if let Some(groups) = summary_root.and_then(|r| r.get("groups")).and_then(|g| g.as_array()) {
        for (idx, g) in groups.iter().enumerate() {
            let group_id = g.get("groupId").and_then(|x| x.as_str()).unwrap_or("");
            let display_name = g
                .get("displayName")
                .or_else(|| g.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim();
            let desc = g.get("description").and_then(|x| x.as_str()).unwrap_or("");
            let model_ids: Vec<String> = g
                .get("modelIds")
                .and_then(|x| x.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();

            let hints = format!("{group_id} {display_name} {desc} {}", model_ids.join(" "));
            let pkey = shared_pool_key(&hints);

            let mut buckets = Vec::new();
            if let Some(raw_buckets) = g.get("buckets").and_then(|b| b.as_array()) {
                for (bidx, b) in raw_buckets.iter().enumerate() {
                    let bid = b
                        .get("bucketId")
                        .or_else(|| b.get("id"))
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("bucket-{idx}-{bidx}"));
                    let win_str = b.get("window").and_then(|x| x.as_str()).unwrap_or("");
                    let (wkey, wlabel) = classify_window(win_str, &bid);
                    let (frac, exhausted) = parse_fraction(b, false);
                    let reset_iso = b
                        .get("resetTime")
                        .or_else(|| b.get("reset_time"))
                        .and_then(|x| x.as_str())
                        .map(str::to_string);
                    let reset_at_ms = parse_reset_ms(reset_iso.as_deref());
                    buckets.push(QuotaBucket {
                        bucket_id: bid,
                        window_key: wkey,
                        window_label: wlabel,
                        remaining_fraction: frac,
                        reset_time_iso: reset_iso,
                        reset_at_ms,
                        is_exhausted: exhausted,
                    });
                }
            }

            let title = if !display_name.is_empty() {
                display_name.to_string()
            } else {
                default_pool_title(&pkey).to_string()
            };

            let entry = pools.entry(pkey.clone()).or_insert_with(|| QuotaPool {
                pool_key: pkey,
                title,
                models: Vec::new(),
                remaining_fraction: None,
                is_exhausted: false,
                reset_time_iso: None,
                reset_at_ms: None,
                buckets: Vec::new(),
            });
            for m in model_ids {
                if !entry.models.contains(&m) {
                    entry.models.push(m);
                }
            }
            entry.buckets.extend(buckets);
        }
    }

    // 2. Merge clientModelConfigs (covers models + fallback quotaInfo when summary lacks buckets)
    if let Some(cfgs) = user_root
        .and_then(|u| u.pointer("/cascadeModelConfigData/clientModelConfigs"))
        .and_then(|c| c.as_array())
    {
        for cfg in cfgs {
            if cfg.get("isHistorical").and_then(|v| v.as_bool()) == Some(true) {
                continue;
            }
            let label = cfg
                .get("label")
                .or_else(|| cfg.get("modelId"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim();
            let model_id = cfg
                .get("modelId")
                .or_else(|| cfg.pointer("/modelOrAlias/model"))
                .and_then(|x| x.as_str())
                .unwrap_or(label);
            if label.is_empty() && model_id.is_empty() {
                continue;
            }
            let pkey = shared_pool_key(&format!("{label} {model_id}"));
            let entry = pools.entry(pkey.clone()).or_insert_with(|| QuotaPool {
                pool_key: pkey.clone(),
                title: default_pool_title(&pkey).to_string(),
                models: Vec::new(),
                remaining_fraction: None,
                is_exhausted: false,
                reset_time_iso: None,
                reset_at_ms: None,
                buckets: Vec::new(),
            });
            let clean_label = strip_effort_suffix(if !label.is_empty() { label } else { model_id });
            if !clean_label.is_empty() && !entry.models.contains(&clean_label) {
                entry.models.push(clean_label);
            }

            if let Some(qi) = cfg.get("quotaInfo") {
                let (frac, exhausted) = parse_fraction(qi, true);
                let reset_iso = qi
                    .get("resetTime")
                    .or_else(|| qi.get("reset_time"))
                    .and_then(|x| x.as_str())
                    .map(str::to_string);
                let reset_ms = parse_reset_ms(reset_iso.as_deref());
                if let Some(f) = frac {
                    if entry.remaining_fraction.map(|cur| f < cur).unwrap_or(true) {
                        entry.remaining_fraction = Some(f);
                        entry.is_exhausted = exhausted;
                    }
                }
                if let Some(rms) = reset_ms {
                    if entry.reset_at_ms.map(|cur| rms < cur).unwrap_or(true) {
                        entry.reset_at_ms = Some(rms);
                        entry.reset_time_iso = reset_iso;
                    }
                }
            }
        }
    }

    // 3. Finalize each pool's tightest bucket and sort pools deterministically
    let mut pool_list: Vec<QuotaPool> = pools
        .into_values()
        .map(|mut p| {
            p.buckets.sort_by_key(|b| window_rank(&b.window_key));
            if let Some(tightest) = p
                .buckets
                .iter()
                .filter(|b| b.remaining_fraction.is_some())
                .min_by(|a, b| {
                    a.remaining_fraction
                        .unwrap_or(1.0)
                        .total_cmp(&b.remaining_fraction.unwrap_or(1.0))
                        .then_with(|| window_rank(&a.window_key).cmp(&window_rank(&b.window_key)))
                })
            {
                p.remaining_fraction = tightest.remaining_fraction;
                p.is_exhausted = tightest.is_exhausted;
                if tightest.reset_at_ms.is_some() {
                    p.reset_at_ms = tightest.reset_at_ms;
                    p.reset_time_iso = tightest.reset_time_iso.clone();
                }
            }
            p
        })
        .collect();

    pool_list.sort_by_key(|p| match p.pool_key.as_str() {
        "shared_gemini_main" => 0,
        "shared_3p_pool" => 1,
        "shared_gemini_argon" => 2,
        "shared_gemini_carbon" => 3,
        _ => 9,
    });

    let primary_fraction = pool_list
        .iter()
        .find(|p| p.pool_key == "shared_gemini_main")
        .and_then(|p| p.remaining_fraction)
        .or_else(|| pool_list.iter().filter_map(|p| p.remaining_fraction).next());

    QuotaSnapshot {
        email,
        plan_name,
        pools: pool_list,
        primary_fraction,
        fetched_at_ms: now_ms,
        source: source.into(),
    }
}

fn strip_effort_suffix(label: &str) -> String {
    let trimmed = label.trim();
    if let Some(idx) = trimmed.rfind('(') {
        if trimmed.ends_with(')') {
            let inner = trimmed[idx + 1..trimmed.len() - 1].to_lowercase();
            if ["low", "medium", "high", "xhigh", "extra high", "thinking"]
                .iter()
                .any(|k| inner.contains(k))
            {
                return trimmed[..idx].trim().to_string();
            }
        }
    }
    trimmed.to_string()
}

/// Fetches account quota directly from Google CloudCode using the account's OAuth access token.
pub async fn fetch_cloudcode_quota(
    access_token: &str,
    email: &str,
    existing_project_id: Option<&str>,
    proxy: Option<&ProxyConfig>,
) -> Result<(QuotaSnapshot, Option<String>, Option<String>), String> {
    let client = build_external_client(proxy, 20)?;
    let os_name = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let arch_name = if cfg!(target_arch = "aarch64") { "arm64" } else { "amd64" };
    let version = detect_antigravity_version();
    let ua = format!("antigravity/{version} {os_name}/{arch_name}");

    let mut project_id = existing_project_id.unwrap_or("").to_string();
    let mut tier_id: Option<String> = None;

    let mut load_body = json!({
        "metadata": {
            "ideName": "antigravity",
            "ideType": "ANTIGRAVITY",
            "pluginType": "GEMINI",
            "updateChannel": "stable"
        },
        "mode": "FULL_ELIGIBILITY_CHECK"
    });
    if !project_id.is_empty() {
        load_body["cloudaicompanionProject"] = json!(project_id);
    }

    if let Ok(resp) = client
        .post(format!("{CLOUDCODE_PROD_URL}/v1internal:loadCodeAssist"))
        .bearer_auth(access_token)
        .header("User-Agent", &ua)
        .json(&load_body)
        .send()
        .await
    {
        if resp.status().as_u16() == 401 || resp.status().as_u16() == 403 {
            return Err(format!("CloudCode 鉴权失效 (HTTP {})", resp.status()));
        }
        if let Ok(v) = resp.json::<Value>().await {
            if let Some(pid) = v
                .get("cloudaicompanionProject")
                .and_then(|p| p.as_str().or_else(|| p.get("id").and_then(|id| id.as_str())))
            {
                if !pid.is_empty() {
                    project_id = pid.to_string();
                }
            }
            tier_id = v
                .pointer("/paidTier/name")
                .or_else(|| v.pointer("/paidTier/id"))
                .or_else(|| v.pointer("/currentTier/name"))
                .or_else(|| v.pointer("/currentTier/id"))
                .and_then(|x| x.as_str())
                .map(str::to_string);
        }
    }

    let payload = if !project_id.is_empty() {
        json!({ "project": project_id })
    } else {
        json!({})
    };

    let (avail_res, summary_res) = tokio::join!(
        client
            .post(format!("{CLOUDCODE_PROD_URL}/v1internal:fetchAvailableModels"))
            .bearer_auth(access_token)
            .header("User-Agent", &ua)
            .json(&payload)
            .send(),
        client
            .post(format!("{CLOUDCODE_PROD_URL}/v1internal:retrieveUserQuotaSummary"))
            .bearer_auth(access_token)
            .header("User-Agent", &ua)
            .json(&payload)
            .send()
    );

    let mut avail_json: Option<Value> = None;
    let mut summary_json: Option<Value> = None;
    let mut auth_status: Option<u16> = None;

    if let Ok(r) = avail_res {
        let st = r.status().as_u16();
        if st == 401 || st == 403 {
            auth_status = Some(st);
        } else if r.status().is_success() {
            avail_json = r.json::<Value>().await.ok();
        }
    }
    if let Ok(r) = summary_res {
        let st = r.status().as_u16();
        if st == 401 || st == 403 {
            auth_status = Some(st);
        } else if r.status().is_success() {
            summary_json = r.json::<Value>().await.ok();
        }
    }

    if avail_json.is_none() && summary_json.is_none() {
        if let Some(st) = auth_status {
            return Err(format!("CloudCode 鉴权失效 (HTTP {st})"));
        }
        return Err("获取 CloudCode 配额失败（网络不可达或代理未连通）".into());
    }

    let mut client_model_configs = Vec::new();
    if let Some(models_map) = avail_json.as_ref().and_then(|v| v.get("models")).and_then(|m| m.as_object()) {
        for (k, v) in models_map {
            let display = v.get("displayName").and_then(|x| x.as_str()).unwrap_or(k);
            let model_val = v.get("model").and_then(|x| x.as_str()).unwrap_or(k);
            client_model_configs.push(json!({
                "modelId": k,
                "label": display,
                "modelOrAlias": { "model": model_val },
                "quotaInfo": v.get("quotaInfo").cloned().unwrap_or(Value::Null)
            }));
        }
    }

    let compat_user_status = json!({
        "userStatus": {
            "email": email,
            "userTier": tier_id.as_ref().map(|t| json!({ "id": t, "name": t })).unwrap_or(Value::Null),
            "cascadeModelConfigData": {
                "clientModelConfigs": client_model_configs
            }
        }
    });

    let snap = parse_quota_snapshot(
        Some(&compat_user_status),
        summary_json.as_ref(),
        Some(email),
        tier_id.as_deref(),
        "cloudcode",
    );

    let final_pid = if project_id.is_empty() { None } else { Some(project_id) };
    let normalized_tier = snap.plan_name.clone();
    Ok((snap, final_pid, normalized_tier))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quota_pools_and_selects_tightest_bucket() {
        let user_status = json!({
            "userStatus": {
                "email": "Dev@Example.com",
                "userTier": { "id": "g1-ultra-tier", "name": "Google AI Ultra" },
                "planStatus": { "planInfo": { "planName": "Pro", "teamsTier": "TEAMS_TIER_PRO_ULTIMATE" } },
                "cascadeModelConfigData": {
                    "clientModelConfigs": [
                        {
                            "modelId": "gemini-4-hydrogen-high",
                            "label": "Gemini 4 Hydrogen (High)",
                            "quotaInfo": { "remainingFraction": 0.72, "resetTime": "2026-11-23T02:00:00Z" }
                        },
                        {
                            "modelId": "claude-opus-5-5",
                            "label": "Claude Opus 5.5",
                            "quotaInfo": { "resetTime": "2026-11-23T05:00:00Z" }
                        }
                    ]
                }
            }
        });
        let summary = json!({
            "groups": [
                {
                    "groupId": "gemini_main",
                    "displayName": "Gemini 4 Hydrogen / Flash",
                    "buckets": [
                        { "bucketId": "b_7d", "window": "weekly", "remainingFraction": 0.85 },
                        { "bucketId": "b_5h", "window": "5h", "remainingFraction": 0.40, "resetTime": "2026-11-23T01:00:00Z" }
                    ]
                }
            ]
        });

        let snap = parse_quota_snapshot(Some(&user_status), Some(&summary), None, None, "daemon");
        assert_eq!(snap.email.as_deref(), Some("dev@example.com"));
        assert_eq!(snap.plan_name.as_deref(), Some("Ultra"));
        assert_eq!(snap.pools.len(), 2);

        let main_pool = &snap.pools[0];
        assert_eq!(main_pool.pool_key, "shared_gemini_main");
        assert_eq!(main_pool.remaining_fraction, Some(0.40));
        assert_eq!(snap.primary_fraction, Some(0.40));

        let claude_pool = &snap.pools[1];
        assert_eq!(claude_pool.pool_key, "shared_3p_pool");
        assert_eq!(claude_pool.remaining_fraction, Some(0.0));
        assert!(claude_pool.is_exhausted);
    }

    #[test]
    fn resolves_ultra_even_when_legacy_plan_name_says_pro() {
        let ultra_by_teams = json!({
            "planStatus": { "planInfo": { "planName": "Pro", "teamsTier": "TEAMS_TIER_PRO_ULTIMATE" } }
        });
        assert_eq!(resolve_plan_badge(Some(&ultra_by_teams), None).as_deref(), Some("Ultra"));

        let free_by_tier = json!({
            "userTier": { "id": "free-tier", "name": "Free" },
            "planStatus": { "planInfo": { "planName": "Pro" } }
        });
        assert_eq!(resolve_plan_badge(Some(&free_by_tier), None).as_deref(), Some("Free"));
    }
}

