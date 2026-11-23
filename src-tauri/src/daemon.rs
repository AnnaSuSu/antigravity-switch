use crate::cloudcode::parse_quota_snapshot;
use crate::models::{QuotaSnapshot, SessionContextSnapshot, SessionListItem, TurnSpeedMetric};
use crate::proxy::build_loopback_client;
use crate::store::normalize_email;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct DaemonHub {
    pub origin: String,
    pub csrf_token: String,
    pub mtime_ms: i64,
}

fn gemini_root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".gemini")
}

fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

/// Discovers running Antigravity LanguageServer instances via `~/.gemini/antigravity/daemon/hub_*.json`.
pub fn discover_daemon_hubs() -> Vec<DaemonHub> {
    let dir = gemini_root().join("antigravity/daemon");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut hubs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("hub_") || !name.ends_with(".json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        let port = v.get("httpsPort").and_then(|x| x.as_u64()).unwrap_or(0);
        let csrf = v
            .get("csrfToken")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if port == 0 || port > 65535 || csrf.is_empty() || !is_pid_alive(pid) {
            continue;
        }
        let mtime_ms = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        hubs.push(DaemonHub {
            origin: format!("https://127.0.0.1:{port}"),
            csrf_token: csrf,
            mtime_ms,
        });
    }
    hubs.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    hubs
}

pub async fn rpc_call(hub: &DaemonHub, method: &str, body: Value) -> Result<Value, String> {
    let client = build_loopback_client(8)?;
    let url = format!("{}/exa.language_server_pb.LanguageServerService/{method}", hub.origin);
    let resp = client
        .post(&url)
        .header("Connect-Protocol-Version", "1")
        .header("x-codeium-csrf-token", &hub.csrf_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("RPC {method} 连接失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("RPC {method} 返回 HTTP {}", resp.status()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("RPC {method} 解析失败: {e}"))
}

pub async fn fetch_daemon_user_email() -> Option<(String, DaemonHub)> {
    for hub in discover_daemon_hubs() {
        if let Ok(v) = rpc_call(&hub, "GetUserStatus", json!({})).await {
            let email = v
                .pointer("/userStatus/email")
                .and_then(|x| x.as_str())
                .map(normalize_email)
                .unwrap_or_default();
            if !email.is_empty() {
                return Some((email, hub));
            }
        }
    }
    None
}

pub async fn fetch_daemon_quota() -> Result<QuotaSnapshot, String> {
    let hubs = discover_daemon_hubs();
    let hub = hubs
        .first()
        .ok_or_else(|| "未检测到运行中的 Antigravity LanguageServer".to_string())?;
    let (user_res, summary_res) = tokio::join!(
        rpc_call(hub, "GetUserStatus", json!({})),
        rpc_call(hub, "RetrieveUserQuotaSummary", json!({}))
    );
    let user_val = user_res.ok();
    let summary_val = summary_res.ok();
    if user_val.is_none() && summary_val.is_none() {
        return Err("本地 LanguageServer 未返回配额数据".into());
    }
    Ok(parse_quota_snapshot(
        user_val.as_ref(),
        summary_val.as_ref(),
        None,
        None,
        "daemon",
    ))
}

/// Lists recently active conversations by scanning `~/.gemini/antigravity/conversations` and `brain`.
pub fn list_recent_sessions(limit: usize) -> Vec<SessionListItem> {
    let mut by_id: std::collections::HashMap<String, SessionListItem> = std::collections::HashMap::new();
    let roots = [
        (gemini_root().join("antigravity"), "desktop"),
        (gemini_root().join("antigravity-cli"), "cli"),
    ];

    for (root, src) in roots {
        let conv_dir = root.join("conversations");
        if let Ok(entries) = fs::read_dir(&conv_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let stem = if let Some(s) = name.strip_suffix(".db") {
                    s
                } else if let Some(s) = name.strip_suffix(".pb") {
                    s
                } else {
                    continue;
                };
                if stem.len() != 36 {
                    continue;
                }
                let mtime_ms = e
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let id = stem.to_lowercase();
                let item = SessionListItem {
                    cascade_id: id.clone(),
                    title: format!("{}…", &id[..8]),
                    modified_at_ms: mtime_ms,
                    source: src.into(),
                };
                by_id
                    .entry(id)
                    .and_modify(|cur| {
                        if mtime_ms > cur.modified_at_ms {
                            cur.modified_at_ms = mtime_ms;
                        }
                    })
                    .or_insert(item);
            }
        }

        let brain_dir = root.join("brain");
        if let Ok(entries) = fs::read_dir(&brain_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.len() != 36 || !e.path().is_dir() {
                    continue;
                }
                let tr = e.path().join(".system_generated/logs/transcript.jsonl");
                let mtime_ms = fs::metadata(&tr)
                    .or_else(|_| e.metadata())
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let id = name.to_lowercase();
                by_id
                    .entry(id.clone())
                    .and_modify(|cur| {
                        if mtime_ms > cur.modified_at_ms {
                            cur.modified_at_ms = mtime_ms;
                        }
                    })
                    .or_insert_with(|| SessionListItem {
                        cascade_id: id.clone(),
                        title: format!("{}…", &id[..8]),
                        modified_at_ms: mtime_ms,
                        source: src.into(),
                    });
            }
        }
    }

    let mut list: Vec<SessionListItem> = by_id.into_values().collect();
    list.sort_by(|a, b| b.modified_at_ms.cmp(&a.modified_at_ms));
    list.truncate(limit);
    list
}

fn parse_duration_sec(v: Option<&Value>) -> Option<f64> {
    let val = v?;
    if let Some(n) = val.as_f64() {
        return (n.is_finite() && n >= 0.0).then_some(n);
    }
    let s = val.as_str()?.trim().trim_end_matches('s');
    let n = s.parse::<f64>().ok()?;
    (n.is_finite() && n >= 0.0).then_some(n)
}

fn num_u64(v: Option<&Value>) -> Option<u64> {
    let val = v?;
    val.as_u64()
        .or_else(|| val.as_i64().and_then(|i| (i >= 0).then_some(i as u64)))
        .or_else(|| val.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
}

fn parse_turn_metric(meta: &Value) -> Option<(TurnSpeedMetric, Option<u64>, Option<u64>, bool)> {
    let cm = meta.get("chatModel")?;
    let model = cm
        .get("modelDisplayName")
        .or_else(|| cm.get("responseModel"))
        .or_else(|| cm.get("model"))
        .and_then(|x| x.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let usage = cm
        .get("retryInfos")
        .and_then(|r| r.as_array())
        .and_then(|a| a.last())
        .and_then(|last| last.get("usage"))
        .or_else(|| cm.get("usage"));

    let input = num_u64(usage.and_then(|u| u.get("inputTokens"))).unwrap_or(0);
    let cached = num_u64(usage.and_then(|u| u.get("cacheReadTokens"))).unwrap_or(0);
    let output = num_u64(usage.and_then(|u| u.get("outputTokens"))).unwrap_or(0);
    let thinking = num_u64(usage.and_then(|u| u.get("thinkingOutputTokens"))).unwrap_or(0);

    let ctx = cm.pointer("/chatStartMetadata/contextWindowMetadata");
    let est = num_u64(ctx.and_then(|c| c.get("estimatedTokensUsed")));
    let budget = num_u64(ctx.and_then(|c| c.get("maxContextTokens")));
    let actual = if input + cached > 0 {
        Some(input + cached + output)
    } else {
        None
    };

    let ttft = parse_duration_sec(cm.get("timeToFirstToken"));
    let streaming = parse_duration_sec(cm.get("streamingDuration"));
    let total_sec = match (ttft, streaming) {
        (Some(t), Some(s)) => Some((t + s * 1.0).max(0.0)),
        (Some(t), None) => Some(t),
        (None, Some(s)) => Some(s),
        (None, None) => None,
    };
    let tps = total_sec
        .filter(|&d| d > 0.0 && output > 0)
        .map(|d| ((output as f64 / d) * 10.0).round() / 10.0);
    let streaming_tps = streaming
        .filter(|&d| d >= 0.05 && output > 0)
        .map(|d| ((output as f64 / d) * 10.0).round() / 10.0);

    let metric = TurnSpeedMetric {
        model,
        input_tokens: input,
        cache_read_tokens: cached,
        output_tokens: output,
        thinking_tokens: thinking,
        ttft_sec: ttft.map(|v| (v * 100.0).round() / 100.0),
        streaming_sec: streaming.map(|v| (v * 100.0).round() / 100.0),
        total_sec: total_sec.map(|v| (v * 100.0).round() / 100.0),
        tps,
        streaming_tps,
    };

    Some((metric, actual.or(est), budget, actual.is_none()))
}

/// Fetches live context window & speed metrics for a given conversation (or most recent active session).
pub async fn fetch_session_context(cascade_id: Option<&str>) -> SessionContextSnapshot {
    let target_id = cascade_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| list_recent_sessions(1).into_iter().next().map(|s| s.cascade_id))
        .unwrap_or_default();

    if target_id.is_empty() {
        return SessionContextSnapshot {
            cascade_id: String::new(),
            model: String::new(),
            used_tokens: None,
            budget_tokens: None,
            percent: None,
            input_tokens: None,
            cache_read_tokens: None,
            output_tokens: None,
            thinking_tokens: None,
            estimated: false,
            latest_speed: None,
            recent_turns: Vec::new(),
            daemon_connected: !discover_daemon_hubs().is_empty(),
            error: Some("未找到本地会话记录".into()),
        };
    }

    let hubs = discover_daemon_hubs();
    for hub in &hubs {
        let Ok(traj) = rpc_call(
            hub,
            "GetCascadeTrajectory",
            json!({ "cascadeId": target_id, "trajectoryVerbosity": 1 }),
        )
        .await
        else {
            continue;
        };

        let gen_count = num_u64(traj.get("numTotalGeneratorMetadata"))
            .or_else(|| {
                traj.pointer("/trajectory/generatorMetadata")
                    .and_then(|a| a.as_array())
                    .map(|a| a.len() as u64)
            })
            .unwrap_or(0);
        let offset = gen_count.saturating_sub(8);

        let Ok(tail) = rpc_call(
            hub,
            "GetCascadeTrajectoryGeneratorMetadata",
            json!({
                "cascadeId": target_id,
                "generatorMetadataOffset": offset,
                "includeMessages": false
            }),
        )
        .await
        else {
            continue;
        };

        let items = tail
            .get("generatorMetadata")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();

        let mut turns = Vec::new();
        let mut latest_used = None;
        let mut latest_budget = None;
        let mut latest_estimated = true;

        for m in &items {
            if let Some((turn, used, budget, estimated)) = parse_turn_metric(m) {
                if used.is_some() {
                    latest_used = used;
                    latest_estimated = estimated;
                }
                if budget.is_some() {
                    latest_budget = budget;
                }
                turns.push(turn);
            }
        }

        let latest = turns.last().cloned();
        let percent = match (latest_used, latest_budget) {
            (Some(u), Some(b)) if b > 0 => Some(((u as f64 / b as f64) * 1000.0).round() / 10.0),
            _ => None,
        };

        return SessionContextSnapshot {
            cascade_id: target_id,
            model: latest.as_ref().map(|l| l.model.clone()).unwrap_or_default(),
            used_tokens: latest_used,
            budget_tokens: latest_budget,
            percent,
            input_tokens: latest.as_ref().map(|l| l.input_tokens),
            cache_read_tokens: latest.as_ref().map(|l| l.cache_read_tokens),
            output_tokens: latest.as_ref().map(|l| l.output_tokens),
            thinking_tokens: latest.as_ref().map(|l| l.thinking_tokens),
            estimated: latest_estimated,
            latest_speed: latest,
            recent_turns: turns.into_iter().rev().collect(),
            daemon_connected: true,
            error: None,
        };
    }

    // Fallback: parse latest turn from local transcript.jsonl when daemon is offline
    if let Some(snap) = read_context_from_transcript(&target_id, !hubs.is_empty()) {
        return snap;
    }

    SessionContextSnapshot {
        cascade_id: target_id,
        model: String::new(),
        used_tokens: None,
        budget_tokens: None,
        percent: None,
        input_tokens: None,
        cache_read_tokens: None,
        output_tokens: None,
        thinking_tokens: None,
        estimated: false,
        latest_speed: None,
        recent_turns: Vec::new(),
        daemon_connected: !hubs.is_empty(),
        error: Some("Antigravity 未运行或该会话暂无模型调用记录".into()),
    }
}

fn read_context_from_transcript(cascade_id: &str, daemon_connected: bool) -> Option<SessionContextSnapshot> {
    let candidates = [
        gemini_root().join(format!("antigravity/brain/{cascade_id}/.system_generated/logs/transcript.jsonl")),
        gemini_root().join(format!("antigravity-cli/brain/{cascade_id}/.system_generated/logs/transcript.jsonl")),
    ];
    let path = candidates.into_iter().find(|p| p.exists())?;
    let content = fs::read_to_string(path).ok()?;
    let mut turns = Vec::new();
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let inp = num_u64(v.get("input_tokens").or_else(|| v.pointer("/chatModel/usage/inputTokens"))).unwrap_or(0);
        let cap = num_u64(v.get("cache_read_tokens").or_else(|| v.pointer("/chatModel/usage/cacheReadTokens"))).unwrap_or(0);
        let outp = num_u64(v.get("output_tokens").or_else(|| v.pointer("/chatModel/usage/outputTokens"))).unwrap_or(0);
        let think = num_u64(v.get("thinking_output_tokens").or_else(|| v.pointer("/chatModel/usage/thinkingOutputTokens"))).unwrap_or(0);
        if inp + cap + outp == 0 {
            continue;
        }
        let model = v
            .pointer("/chatModel/modelDisplayName")
            .or_else(|| v.pointer("/chatModel/model"))
            .or_else(|| v.get("model"))
            .and_then(|x| x.as_str())
            .unwrap_or("Unknown")
            .to_string();
        turns.push(TurnSpeedMetric {
            model,
            input_tokens: inp,
            cache_read_tokens: cap,
            output_tokens: outp,
            thinking_tokens: think,
            ttft_sec: None,
            streaming_sec: None,
            total_sec: None,
            tps: None,
            streaming_tps: None,
        });
    }
    let latest = turns.last()?.clone();
    let used = latest.input_tokens + latest.cache_read_tokens + latest.output_tokens;
    Some(SessionContextSnapshot {
        cascade_id: cascade_id.to_string(),
        model: latest.model.clone(),
        used_tokens: Some(used),
        budget_tokens: None,
        percent: None,
        input_tokens: Some(latest.input_tokens),
        cache_read_tokens: Some(latest.cache_read_tokens),
        output_tokens: Some(latest.output_tokens),
        thinking_tokens: Some(latest.thinking_tokens),
        estimated: false,
        latest_speed: Some(latest),
        recent_turns: turns.into_iter().rev().take(8).collect(),
        daemon_connected,
        error: if daemon_connected { None } else { Some("离线模式（读取本地记录，无实时速度）".into()) },
    })
}

/// Restarts only Antigravity's `language_server` processes so newly written credentials take effect
/// without killing the user's Electron UI window.
pub fn restart_language_servers() -> Result<Vec<String>, String> {
    let mut killed = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let out = Command::new("/bin/ps")
            .args(["-ax", "-o", "pid=,comm="])
            .output()
            .map_err(|e| format!("查询进程失败: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let trimmed = line.trim();
            let Some((pid_str, comm)) = trimmed.split_once(char::is_whitespace) else {
                continue;
            };
            let comm = comm.trim();
            let is_target = comm.ends_with("/Antigravity.app/Contents/Resources/bin/language_server")
                || comm.ends_with("/Antigravity IDE.app/Contents/Resources/bin/language_server")
                || comm.ends_with("/Google Antigravity.app/Contents/Resources/bin/language_server")
                || comm.ends_with("/language_server_darwin_arm64")
                || comm.ends_with("/language_server_darwin_x64");
            if is_target {
                let _ = Command::new("/bin/kill").args(["-TERM", pid_str]).status();
                killed.push(format!("language_server#{pid_str}"));
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        for img in ["language_server.exe", "language_server_windows_x64.exe"] {
            if let Ok(st) = Command::new("taskkill.exe").args(["/F", "/IM", img]).status() {
                if st.success() {
                    killed.push(img.to_string());
                }
            }
        }
    }

    Ok(killed)
}
