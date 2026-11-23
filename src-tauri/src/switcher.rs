use crate::cloudcode::fetch_cloudcode_quota;
use crate::credentials::{
    apply_credential, capture_snapshot, restore_snapshot, CredentialPaths, SystemCredentialInput,
};
use crate::daemon::{discover_daemon_hubs, fetch_daemon_user_email, restart_language_servers};
use crate::models::{AccountRecord, AppStoreData, QuotaSnapshot, SwitchLog, SwitchResult};
use crate::oauth::refresh_access_token;
use crate::proxy::launch_antigravity_with_proxy;
use crate::store::{load_store, normalize_email, save_store};
use std::time::Duration;

fn is_token_expiring_soon(expires_at: &str) -> bool {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return true;
    };
    dt.with_timezone(&chrono::Utc) <= chrono::Utc::now() + chrono::Duration::minutes(5)
}

/// Ensures an account has a fresh access token, updating the account record in place.
pub async fn ensure_fresh_token(
    acct: &mut AccountRecord,
    proxy: &crate::models::ProxyConfig,
) -> Result<(), String> {
    if !acct.access_token.is_empty() && !is_token_expiring_soon(&acct.expires_at) {
        return Ok(());
    }
    if acct.refresh_token.is_empty() {
        return Ok(());
    }
    let fresh = refresh_access_token(
        &acct.refresh_token,
        acct.oauth_client_id.as_deref(),
        Some(proxy),
    )
    .await?;
    acct.access_token = fresh.access_token;
    acct.refresh_token = fresh.refresh_token;
    acct.expires_at = fresh.expires_at;
    if !fresh.id_token.is_empty() {
        acct.id_token = fresh.id_token;
    }
    acct.oauth_client_id = Some(fresh.client_id);
    Ok(())
}

/// Refreshes CloudCode quota for a single stored account.
pub async fn refresh_single_account_quota(email: &str) -> Result<QuotaSnapshot, String> {
    let key = normalize_email(email);
    let mut store = load_store();
    let proxy = store.proxy.clone();
    let mut acct = store
        .accounts
        .get(&key)
        .cloned()
        .ok_or_else(|| format!("账号不存在: {key}"))?;

    if let Err(e) = ensure_fresh_token(&mut acct, &proxy).await {
        acct.last_error = Some(e.clone());
        store.accounts.insert(key, acct);
        let _ = save_store(&store);
        return Err(e);
    }

    match fetch_cloudcode_quota(&acct.access_token, &acct.email, acct.project_id.as_deref(), Some(&proxy)).await {
        Ok((snap, pid, tier)) => {
            acct.project_id = pid.or(acct.project_id);
            acct.plan_name = tier.or_else(|| snap.plan_name.clone()).or(acct.plan_name);
            acct.last_quota = Some(snap.clone());
            acct.last_error = None;
            store.accounts.insert(key, acct);
            let _ = save_store(&store);
            Ok(snap)
        }
        Err(e) => {
            acct.last_error = Some(e.clone());
            store.accounts.insert(key, acct);
            let _ = save_store(&store);
            Err(e)
        }
    }
}

/// Atomically switches Antigravity Desktop + IDE + CLI credentials with auto-rollback on verification failure.
pub async fn switch_account(target_email: &str, trigger: &str) -> SwitchResult {
    let target = normalize_email(target_email);
    let mut store = load_store();
    let proxy = store.proxy.clone();
    let prev_email = store.active_email.clone();

    let Some(mut acct) = store.accounts.get(&target).cloned() else {
        return SwitchResult {
            ok: false,
            email: target,
            verified: false,
            rolled_back: false,
            message: "目标账号不存在".into(),
        };
    };

    if let Err(e) = ensure_fresh_token(&mut acct, &proxy).await {
        return SwitchResult {
            ok: false,
            email: target,
            verified: false,
            rolled_back: false,
            message: format!("刷新目标账号 Token 失败: {e}"),
        };
    }

    let paths = CredentialPaths::detect();
    let snapshot = capture_snapshot(&paths);

    let cred_input = SystemCredentialInput {
        email: acct.email.clone(),
        name: acct.name.clone(),
        access_token: acct.access_token.clone(),
        refresh_token: acct.refresh_token.clone(),
        expires_at: acct.expires_at.clone(),
        id_token: acct.id_token.clone(),
    };

    if let Err(e) = apply_credential(&paths, &cred_input) {
        let rolled_back = restore_snapshot(&paths, &snapshot).is_ok();
        let msg = format!("写入系统凭据失败: {e}（已自动回滚）");
        record_switch_log(&mut store, prev_email, target.clone(), trigger, false, &msg);
        return SwitchResult {
            ok: false,
            email: target,
            verified: false,
            rolled_back,
            message: msg,
        };
    }

    let daemon_was_running = !discover_daemon_hubs().is_empty();
    let mut verified = false;

    if daemon_was_running {
        let _ = restart_language_servers();
        // Wait for Antigravity host auto-restart (~2s) + LS initialization, polling GetUserStatus up to 15s.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        tokio::time::sleep(Duration::from_millis(800)).await;
        while tokio::time::Instant::now() < deadline {
            if let Some((live_email, _)) = fetch_daemon_user_email().await {
                if live_email.eq_ignore_ascii_case(&target) {
                    verified = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(600)).await;
        }

        if !verified {
            let rolled_back = restore_snapshot(&paths, &snapshot).is_ok();
            let _ = restart_language_servers();
            let msg = format!("切换核验超时：本地 LanguageServer 未生效为 {target}，已自动回滚原凭据");
            record_switch_log(&mut store, prev_email, target.clone(), trigger, false, &msg);
            return SwitchResult {
                ok: false,
                email: target,
                verified: false,
                rolled_back,
                message: msg,
            };
        }
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    acct.last_used_at_ms = now_ms;
    store.active_email = Some(target.clone());
    store.accounts.insert(target.clone(), acct);

    let restarted_app = if store.switch_restarts_app {
        launch_antigravity_with_proxy(&proxy, true).is_ok()
    } else {
        false
    };

    let msg = if restarted_app {
        format!("已切换至 {target} 并重启 Antigravity")
    } else if verified {
        format!("已切换至 {target} 并核验生效")
    } else {
        format!("已写入 {target} 凭据（Antigravity 未运行，下次启动即生效）")
    };
    record_switch_log(&mut store, prev_email, target.clone(), trigger, true, &msg);

    SwitchResult {
        ok: true,
        email: target,
        verified,
        rolled_back: false,
        message: msg,
    }
}

fn record_switch_log(
    store: &mut AppStoreData,
    from_email: Option<String>,
    to_email: String,
    trigger: &str,
    ok: bool,
    message: &str,
) {
    store.last_switch = Some(SwitchLog {
        at_ms: chrono::Utc::now().timestamp_millis(),
        from_email,
        to_email,
        trigger: trigger.into(),
        ok,
        message: message.into(),
    });
    let _ = save_store(store);
}

pub fn extract_pool_remaining(snap: &QuotaSnapshot, pool_key: &str, window_key: &str) -> Option<f64> {
    let pool = snap.pools.iter().find(|p| p.pool_key == pool_key)?;
    if window_key == "primary" {
        return pool.remaining_fraction;
    }
    pool.buckets
        .iter()
        .find(|b| b.window_key == window_key)
        .and_then(|b| b.remaining_fraction)
        .or(pool.remaining_fraction)
}

/// Checks if the active account breached the auto-switch threshold. Candidate accounts' remote
/// quotas are fetched on demand only after the active account actually crosses the threshold.
pub async fn evaluate_auto_switch() -> Option<SwitchResult> {
    let store = load_store();
    let cfg = store.auto_switch.clone();
    if !cfg.enabled {
        return None;
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    if let Some(last) = &store.last_switch {
        let elapsed_sec = ((now_ms - last.at_ms).max(0) / 1000) as u64;
        if elapsed_sec < cfg.cooldown_sec {
            return None;
        }
    }

    let active_email = store.active_email.clone()?;
    let active_acct = store.accounts.get(&active_email)?;
    let active_snap = active_acct.last_quota.as_ref()?;
    let active_frac = extract_pool_remaining(active_snap, &cfg.pool_key, &cfg.window_key)?;
    let active_pct = active_frac * 100.0;

    if active_pct > cfg.threshold_pct as f64 {
        return None;
    }

    let candidate_emails: Vec<String> = store
        .accounts
        .iter()
        .filter(|(email, acct)| {
            !email.eq_ignore_ascii_case(&active_email)
                && !acct.refresh_token.is_empty()
                && (cfg.candidates.is_empty()
                    || cfg.candidates.iter().any(|c| c.eq_ignore_ascii_case(email)))
        })
        .map(|(email, _)| email.clone())
        .collect();

    let mut best: Option<(String, f64)> = None;
    for email in candidate_emails {
        let snap = match refresh_single_account_quota(&email).await {
            Ok(s) => s,
            Err(_) => continue,
        };
        let Some(frac) = extract_pool_remaining(&snap, &cfg.pool_key, &cfg.window_key) else {
            continue;
        };
        let pct = frac * 100.0;
        if pct > cfg.threshold_pct as f64 && best.as_ref().map(|b| pct > b.1).unwrap_or(true) {
            best = Some((email, pct));
        }
    }

    let (target_email, _) = best?;
    Some(switch_account(&target_email, "auto").await)
}
