mod cloudcode;
mod credentials;
mod daemon;
mod models;
mod oauth;
mod popover;
mod protobuf;
mod proxy;
mod store;
mod switcher;
mod usage;

use crate::cloudcode::fetch_cloudcode_quota;
use crate::credentials::{import_current_credential, CredentialPaths};
use crate::daemon::{
    discover_daemon_hubs, fetch_daemon_quota, fetch_daemon_user_email, fetch_session_context,
    list_recent_sessions,
};
use crate::models::{
    AccountRecord, AutoSwitchConfig, DashboardOverview, ProxyConfig, ProxyTestResult,
    SessionContextSnapshot, SessionListItem, SwitchResult, TrayDisplayConfig, UsageSummary,
};
use crate::oauth::{cancel_oauth_login, fetch_userinfo, run_oauth_browser_login};
use crate::proxy::{
    detect_antigravity_app_path, launch_antigravity_with_proxy, test_proxy_connection,
    validate_proxy_url,
};
use crate::store::{load_store, normalize_email, save_store};
use crate::switcher::{evaluate_auto_switch, refresh_single_account_quota, switch_account};
use crate::usage::scan_local_usage;
use std::sync::Mutex;
#[cfg(not(target_os = "macos"))]
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

const TRAY_ID: &str = "main-tray";

#[derive(Default, Clone)]
struct TrayMetricsCache {
    quota_pct: Option<f64>,
    ctx_tokens: Option<u64>,
    tps: Option<f64>,
}

static TRAY_METRICS: Mutex<TrayMetricsCache> = Mutex::new(TrayMetricsCache {
    quota_pct: None,
    ctx_tokens: None,
    tps: None,
});

fn format_compact_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}K", (n as f64 / 1_000.0).round() as u64)
    } else {
        n.to_string()
    }
}

fn render_tray_from_cache(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let display = load_store().tray_display;
    let metrics = TRAY_METRICS
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();

    let mut parts = Vec::new();
    if display.show_quota {
        if let Some(q) = metrics.quota_pct {
            parts.push(format!("{}%", (q * 100.0).round() as i64));
        }
    }
    if display.show_context {
        if let Some(c) = metrics.ctx_tokens {
            parts.push(format_compact_tokens(c));
        }
    }
    if display.show_speed {
        if let Some(s) = metrics.tps.filter(|&v| v > 0.0) {
            parts.push(format!("{:.0}t/s", s));
        }
    }
    let summary = parts.join(" · ");

    #[cfg(target_os = "macos")]
    {
        let title = if summary.is_empty() {
            String::new()
        } else {
            format!(" {summary}")
        };
        let _ = tray.set_title(Some(title));
        popover::sync_tray_hit_area(app, TRAY_ID);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let tooltip = if summary.is_empty() {
            "Antigravity Switch".to_string()
        } else {
            format!("Antigravity Switch — {summary}")
        };
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

fn update_tray_metrics(app: &AppHandle, quota_pct: Option<f64>, ctx_tokens: Option<u64>, tps: Option<f64>) {
    if let Ok(mut guard) = TRAY_METRICS.lock() {
        if quota_pct.is_some() {
            guard.quota_pct = quota_pct;
        }
        if ctx_tokens.is_some() {
            guard.ctx_tokens = ctx_tokens;
        }
        if tps.is_some() {
            guard.tps = tps;
        }
    }
    render_tray_from_cache(app);
}

#[tauri::command]
async fn get_overview(app: AppHandle) -> Result<DashboardOverview, String> {
    let mut store = load_store();
    let hubs = discover_daemon_hubs();
    let daemon_online = !hubs.is_empty();

    let daemon_quota = if daemon_online {
        fetch_daemon_quota().await.ok()
    } else {
        None
    };

    let daemon_email = match daemon_quota.as_ref().and_then(|q| q.email.clone()) {
        Some(e) => Some(e),
        None if daemon_online => fetch_daemon_user_email().await.map(|(e, _)| e),
        None => None,
    };

    if let Some(ref email) = daemon_email {
        let norm = normalize_email(email);
        if store.accounts.contains_key(&norm) {
            store.active_email = Some(norm.clone());
            if let Some(ref q) = daemon_quota {
                if let Some(acct) = store.accounts.get_mut(&norm) {
                    acct.last_quota = Some(q.clone());
                    if let Some(plan) = &q.plan_name {
                        acct.plan_name = Some(plan.clone());
                    }
                    acct.last_error = None;
                }
            }
            let _ = save_store(&store);
        } else if store.active_email.is_none() {
            store.active_email = Some(norm);
        }
    }

    let effective_active = daemon_email.clone().or_else(|| store.active_email.clone());
    let active_quota = daemon_quota.or_else(|| {
        effective_active
            .as_deref()
            .and_then(|e| store.accounts.get(e))
            .and_then(|a| a.last_quota.clone())
    });

    update_tray_metrics(
        &app,
        active_quota.as_ref().and_then(|q| q.primary_fraction),
        None,
        None,
    );

    let accounts = store
        .accounts
        .values()
        .map(|a| a.to_summary(effective_active.as_deref()))
        .collect();

    Ok(DashboardOverview {
        daemon_online,
        daemon_email,
        active_email: effective_active,
        accounts,
        active_quota,
        auto_switch: store.auto_switch,
        proxy: store.proxy,
        tray_display: store.tray_display,
        switch_restarts_app: store.switch_restarts_app,
        antigravity_app_path: detect_antigravity_app_path().map(|p| p.to_string_lossy().into_owned()),
        last_switch: store.last_switch,
    })
}

#[tauri::command]
async fn import_current_account(app: AppHandle) -> Result<DashboardOverview, String> {
    let paths = CredentialPaths::detect();
    let imported = import_current_credential(&paths)?;
    let mut store = load_store();
    let proxy = store.proxy.clone();

    let mut email = normalize_email(&imported.email);
    let mut name = imported.name.clone();

    if email.is_empty() && !imported.access_token.is_empty() {
        if let Ok(info) = fetch_userinfo(&imported.access_token, Some(&proxy)).await {
            email = info.email;
            name = info.name;
        }
    }
    if email.is_empty() {
        if let Some((live_email, _)) = fetch_daemon_user_email().await {
            email = live_email;
        }
    }
    if email.is_empty() {
        return Err("已读取到本机凭据但无法识别账号邮箱，请使用「登录添加」".into());
    }
    if name.is_empty() {
        name = email.split('@').next().unwrap_or("User").to_string();
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let existing = store.accounts.get(&email).cloned();
    let mut rec = AccountRecord {
        email: email.clone(),
        name,
        access_token: imported.access_token,
        refresh_token: imported.refresh_token,
        expires_at: imported.expires_at,
        id_token: imported.id_token,
        oauth_client_id: existing.as_ref().and_then(|e| e.oauth_client_id.clone()),
        project_id: existing.as_ref().and_then(|e| e.project_id.clone()),
        plan_name: existing.as_ref().and_then(|e| e.plan_name.clone()),
        last_quota: existing.as_ref().and_then(|e| e.last_quota.clone()),
        last_error: None,
        added_at_ms: existing.as_ref().map(|e| e.added_at_ms).unwrap_or(now_ms),
        last_used_at_ms: now_ms,
    };

    if let Ok((snap, pid, tier)) =
        fetch_cloudcode_quota(&rec.access_token, &rec.email, rec.project_id.as_deref(), Some(&proxy)).await
    {
        rec.project_id = pid.or(rec.project_id);
        rec.plan_name = tier.or_else(|| snap.plan_name.clone()).or(rec.plan_name);
        rec.last_quota = Some(snap);
    }

    store.active_email = Some(email.clone());
    store.accounts.insert(email, rec);
    save_store(&store)?;

    get_overview(app).await
}

#[tauri::command]
async fn oauth_add_account(app: AppHandle) -> Result<DashboardOverview, String> {
    let mut store = load_store();
    let proxy = store.proxy.clone();
    let (tokens, user) = run_oauth_browser_login(Some(&proxy)).await?;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let email = normalize_email(&user.email);
    let existing = store.accounts.get(&email).cloned();

    let mut rec = AccountRecord {
        email: email.clone(),
        name: user.name,
        access_token: tokens.access_token,
        refresh_token: if tokens.refresh_token.is_empty() {
            existing.as_ref().map(|e| e.refresh_token.clone()).unwrap_or_default()
        } else {
            tokens.refresh_token
        },
        expires_at: tokens.expires_at,
        id_token: tokens.id_token,
        oauth_client_id: Some(tokens.client_id),
        project_id: existing.as_ref().and_then(|e| e.project_id.clone()),
        plan_name: existing.as_ref().and_then(|e| e.plan_name.clone()),
        last_quota: existing.as_ref().and_then(|e| e.last_quota.clone()),
        last_error: None,
        added_at_ms: existing.as_ref().map(|e| e.added_at_ms).unwrap_or(now_ms),
        last_used_at_ms: existing.as_ref().map(|e| e.last_used_at_ms).unwrap_or(0),
    };

    if let Ok((snap, pid, tier)) =
        fetch_cloudcode_quota(&rec.access_token, &rec.email, rec.project_id.as_deref(), Some(&proxy)).await
    {
        rec.project_id = pid.or(rec.project_id);
        rec.plan_name = tier.or_else(|| snap.plan_name.clone()).or(rec.plan_name);
        rec.last_quota = Some(snap);
    }

    store.accounts.insert(email, rec);
    save_store(&store)?;

    // Re-show popover so the user sees the newly added account right after finishing browser OAuth.
    popover::show_popover(&app, TRAY_ID, None, false);
    get_overview(app).await
}

#[tauri::command]
fn cancel_oauth_login_cmd() -> bool {
    cancel_oauth_login()
}

#[tauri::command]
async fn remove_account(app: AppHandle, email: String) -> Result<DashboardOverview, String> {
    let key = normalize_email(&email);
    let mut store = load_store();
    store.accounts.remove(&key);
    store.auto_switch.candidates.retain(|c| !c.eq_ignore_ascii_case(&key));
    if store.active_email.as_deref().map(|a| a.eq_ignore_ascii_case(&key)).unwrap_or(false) {
        store.active_email = None;
    }
    save_store(&store)?;
    get_overview(app).await
}

#[tauri::command]
async fn switch_account_cmd(app: AppHandle, email: String) -> Result<SwitchResult, String> {
    let res = switch_account(&email, "manual").await;
    if res.ok {
        let _ = refresh_single_account_quota(&email).await;
        let _ = get_overview(app).await;
    }
    Ok(res)
}

#[tauri::command]
async fn refresh_all_quotas(app: AppHandle) -> Result<DashboardOverview, String> {
    let store = load_store();
    let emails: Vec<String> = store.accounts.keys().cloned().collect();
    for email in emails {
        let _ = refresh_single_account_quota(&email).await;
    }
    let _ = evaluate_auto_switch().await;
    get_overview(app).await
}

#[tauri::command]
async fn save_auto_switch_config(app: AppHandle, config: AutoSwitchConfig) -> Result<DashboardOverview, String> {
    let mut store = load_store();
    store.auto_switch = AutoSwitchConfig {
        enabled: config.enabled,
        pool_key: config.pool_key,
        window_key: config.window_key,
        threshold_pct: config.threshold_pct.clamp(1, 95),
        cooldown_sec: config.cooldown_sec.clamp(30, 3600),
        candidates: config.candidates,
    };
    save_store(&store)?;
    get_overview(app).await
}

#[tauri::command]
async fn save_tray_display_config(app: AppHandle, config: TrayDisplayConfig) -> Result<DashboardOverview, String> {
    let mut store = load_store();
    store.tray_display = config;
    save_store(&store)?;
    render_tray_from_cache(&app);
    get_overview(app).await
}

#[tauri::command]
async fn save_switch_behavior(app: AppHandle, restart_app: bool) -> Result<DashboardOverview, String> {
    let mut store = load_store();
    store.switch_restarts_app = restart_app;
    save_store(&store)?;
    get_overview(app).await
}

#[tauri::command]
fn set_window_pinned(pinned: bool) -> bool {
    popover::set_pinned(pinned)
}

#[tauri::command]
fn set_popover_height(app: AppHandle, height: f64) {
    popover::set_card_height(&app, height);
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
async fn list_sessions() -> Result<Vec<SessionListItem>, String> {
    tokio::task::spawn_blocking(|| list_recent_sessions(20))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_session_context(app: AppHandle, cascade_id: Option<String>) -> Result<SessionContextSnapshot, String> {
    let snap = fetch_session_context(cascade_id.as_deref()).await;
    update_tray_metrics(
        &app,
        None,
        snap.used_tokens,
        snap.latest_speed.as_ref().and_then(|s| s.streaming_tps.or(s.tps)),
    );
    Ok(snap)
}

#[tauri::command]
async fn get_usage_summary() -> Result<UsageSummary, String> {
    tokio::task::spawn_blocking(scan_local_usage)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_proxy_config(app: AppHandle, config: ProxyConfig) -> Result<DashboardOverview, String> {
    if config.enabled {
        validate_proxy_url(&config.proxy_url)?;
    }
    let mut store = load_store();
    store.proxy = config;
    save_store(&store)?;
    get_overview(app).await
}

#[tauri::command]
async fn test_proxy(proxy_url: String) -> Result<ProxyTestResult, String> {
    Ok(test_proxy_connection(&proxy_url).await)
}

#[tauri::command]
async fn launch_antigravity_cmd(restart_if_running: bool) -> Result<String, String> {
    let store = load_store();
    let proxy = store.proxy.clone();
    tokio::task::spawn_blocking(move || launch_antigravity_with_proxy(&proxy, restart_if_running))
        .await
        .map_err(|e| e.to_string())?
}

fn spawn_background_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let mut primary_frac = None;

            // Read only local LanguageServer during idle polling (zero external Google requests).
            if let Ok(dq) = fetch_daemon_quota().await {
                primary_frac = dq.primary_fraction;
                if let Some(ref email) = dq.email {
                    let norm = normalize_email(email);
                    let mut st = load_store();
                    if let Some(acct) = st.accounts.get_mut(&norm) {
                        acct.last_quota = Some(dq.clone());
                        if let Some(plan) = &dq.plan_name {
                            acct.plan_name = Some(plan.clone());
                        }
                        st.active_email = Some(norm);
                        let _ = save_store(&st);
                    }
                }
            }

            if load_store().auto_switch.enabled {
                let _ = evaluate_auto_switch().await;
            }

            let sess = fetch_session_context(None).await;
            update_tray_metrics(
                &app,
                primary_frac,
                sess.used_tokens,
                sess.latest_speed.as_ref().and_then(|s| s.streaming_tps.or(s.tps)),
            );
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let silent_start = std::env::args().any(|a| a == "--silent" || a == "--background");

    tauri::Builder::default()
        .setup(move |app| {
            // Set Accessory policy before event loop launch so the app never enters the Dock.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))
                .unwrap_or_else(|_| app.default_window_icon().unwrap().clone());

            let mut builder = TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        let accepted = if cfg!(target_os = "macos") {
                            matches!(button, MouseButton::Left | MouseButton::Right)
                        } else {
                            button == MouseButton::Left
                        };
                        if accepted {
                            popover::toggle_popover(tray.app_handle(), TRAY_ID, Some(rect));
                        }
                    }
                });

            #[cfg(target_os = "macos")]
            {
                // Omit native menu on macOS so both left and right clicks cleanly toggle the popover.
                builder = builder.icon_as_template(true);
            }

            #[cfg(not(target_os = "macos"))]
            {
                let show_item = MenuItem::with_id(app, "toggle", "显示 / 隐藏面板", true, None::<&str>)?;
                let refresh_item = MenuItem::with_id(app, "refresh", "刷新配额", true, None::<&str>)?;
                let launch_item = MenuItem::with_id(app, "launch_proxy", "启动 Antigravity", true, None::<&str>)?;
                let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show_item, &refresh_item, &launch_item, &quit_item])?;

                builder = builder
                    .menu(&menu)
                    .tooltip("Antigravity Switch")
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "toggle" => popover::toggle_popover(app, TRAY_ID, None),
                        "refresh" => {
                            let handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = refresh_all_quotas(handle).await;
                            });
                        }
                        "launch_proxy" => {
                            let proxy = load_store().proxy;
                            let _ = launch_antigravity_with_proxy(&proxy, false);
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    });
            }

            let _tray = builder.build(app)?;
            popover::sync_tray_hit_area(app.handle(), TRAY_ID);
            spawn_background_poller(app.handle().clone());

            // Show after NSApplication finishes launching so tao's activation hack and status item layout settle first.
            if !silent_start {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
                    let rect = handle
                        .tray_by_id(TRAY_ID)
                        .and_then(|t| t.rect().ok().flatten());
                    popover::show_popover(&handle, TRAY_ID, rect, false);
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Focused(false) => {
                if let Some(win) = window.app_handle().get_webview_window(window.label()) {
                    popover::on_window_blur(&win);
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            get_overview,
            import_current_account,
            oauth_add_account,
            cancel_oauth_login_cmd,
            remove_account,
            switch_account_cmd,
            refresh_all_quotas,
            save_auto_switch_config,
            save_tray_display_config,
            save_switch_behavior,
            set_window_pinned,
            set_popover_height,
            quit_app,
            list_sessions,
            get_session_context,
            get_usage_summary,
            save_proxy_config,
            test_proxy,
            launch_antigravity_cmd,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Antigravity Switch")
        .run(|_app, event| {
            if let RunEvent::ExitRequested { api, code, .. } = event {
                // Keep tray resident alive unless exit was explicitly requested via app.exit().
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
