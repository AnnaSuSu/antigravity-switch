use crate::models::{DailyUsageRow, ModelUsageRow, TokenTotals, UsageSummary};
use crate::protobuf::extract_gen_metadata_models;
use chrono::{DateTime, Duration, Local};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

fn gemini_root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".gemini")
}

#[derive(Default)]
struct DayAcc {
    desktop_total: u64,
    cli_total: u64,
    input: u64,
    cache_read: u64,
    output: u64,
    thinking: u64,
    requests: u64,
}

#[derive(Default)]
struct ModelAcc {
    input: u64,
    cache_read: u64,
    output: u64,
    requests: u64,
}

fn calc_totals(acc: &mut TokenTotals) {
    let denom = acc.input + acc.cache_read;
    acc.cache_hit_ratio = if denom > 0 {
        ((acc.cache_read as f64 / denom as f64) * 1000.0).round() / 10.0
    } else {
        0.0
    };
}

fn add_to_totals(t: &mut TokenTotals, inp: u64, cap: u64, outp: u64, think: u64) {
    t.input += inp;
    t.cache_read += cap;
    t.output += outp;
    t.thinking += think;
    t.total += inp + cap + outp;
    t.requests += 1;
}

fn num_u64(v: Option<&Value>) -> u64 {
    v.and_then(|val| {
        val.as_u64()
            .or_else(|| val.as_i64().and_then(|i| (i >= 0).then_some(i as u64)))
            .or_else(|| val.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
    })
    .unwrap_or(0)
}

/// Scans local Desktop and CLI transcripts (with WAL-safe conversation DB model backfill).
pub fn scan_local_usage() -> UsageSummary {
    let now_local = Local::now();
    let today_str = now_local.format("%Y-%m-%d").to_string();
    let cutoff_7d = (now_local - Duration::days(6)).format("%Y-%m-%d").to_string();
    let cutoff_30d = (now_local - Duration::days(29)).format("%Y-%m-%d").to_string();

    let sources = [
        (gemini_root().join("antigravity"), "desktop"),
        (gemini_root().join("antigravity-cli"), "cli"),
    ];

    let mut seen_event_ids: HashSet<String> = HashSet::new();
    let mut daily_map: BTreeMap<String, DayAcc> = BTreeMap::new();
    let mut model_map: HashMap<String, ModelAcc> = HashMap::new();
    let mut db_model_cache: HashMap<PathBuf, HashMap<i64, String>> = HashMap::new();

    let mut today = TokenTotals::default();
    let mut last_7d = TokenTotals::default();
    let mut last_30d = TokenTotals::default();
    let mut all_time = TokenTotals::default();
    let mut desktop_total = 0u64;
    let mut cli_total = 0u64;

    for (root, source_name) in sources {
        if !root.exists() {
            continue;
        }
        let conv_dir = root.join("conversations");
        let mut transcript_files = Vec::new();
        collect_transcript_files(&root, 0, &mut transcript_files);

        for tr_path in transcript_files {
            let Ok(content) = fs::read_to_string(&tr_path) else {
                continue;
            };
            let inferred_cascade = extract_brain_cascade_id(&tr_path);

            for (line_idx, line) in content.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(ev) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let Some(created_raw) = ev.get("created_at").and_then(|x| x.as_str()) else {
                    continue;
                };
                let Ok(dt) = DateTime::parse_from_rfc3339(created_raw) else {
                    continue;
                };
                let date_key = dt.with_timezone(&Local).format("%Y-%m-%d").to_string();

                let (inp, cap, outp, think) = if ev.get("input_tokens").is_some()
                    || ev.get("output_tokens").is_some()
                    || ev.get("cache_read_tokens").is_some()
                {
                    (
                        num_u64(ev.get("input_tokens")),
                        num_u64(ev.get("cache_read_tokens")),
                        num_u64(ev.get("output_tokens")),
                        num_u64(ev.get("thinking_output_tokens")),
                    )
                } else if let Some(u) = ev.pointer("/chatModel/usage") {
                    (
                        num_u64(u.get("inputTokens")),
                        num_u64(u.get("cacheReadTokens")),
                        num_u64(u.get("outputTokens")),
                        num_u64(u.get("thinkingOutputTokens")),
                    )
                } else {
                    (0, 0, 0, 0)
                };

                let tot = inp + cap + outp;
                if tot == 0 {
                    continue;
                }

                let cascade_id = ev
                    .get("cascadeId")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .or_else(|| inferred_cascade.clone())
                    .unwrap_or_else(|| tr_path.to_string_lossy().into_owned());
                let step_idx = ev
                    .get("step_index")
                    .or_else(|| ev.get("stepIndex"))
                    .and_then(|x| x.as_i64());

                let event_id = ev
                    .pointer("/chatModel/usage/responseId")
                    .or_else(|| ev.get("responseId"))
                    .or_else(|| ev.get("response_id"))
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("resp:{s}"))
                    .unwrap_or_else(|| format!("{source_name}:{cascade_id}#{}", step_idx.unwrap_or(line_idx as i64)));

                if !seen_event_ids.insert(event_id) {
                    continue;
                }

                let mut model = ev
                    .pointer("/chatModel/modelDisplayName")
                    .or_else(|| ev.pointer("/chatModel/model"))
                    .or_else(|| ev.pointer("/chatModel/responseModel"))
                    .or_else(|| ev.get("model"))
                    .or_else(|| ev.get("modelId"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();

                if model.is_empty() {
                    if let Some(si) = step_idx {
                        let db_path = conv_dir.join(format!("{cascade_id}.db"));
                        if db_path.exists() {
                            let step_map = db_model_cache
                                .entry(db_path.clone())
                                .or_insert_with(|| load_step_models_readonly(&db_path));
                            if let Some(m) = step_map.get(&si) {
                                model = m.clone();
                            }
                        }
                    }
                }
                let model_display = normalize_model_label(&model);

                add_to_totals(&mut all_time, inp, cap, outp, think);
                if date_key == today_str {
                    add_to_totals(&mut today, inp, cap, outp, think);
                }
                if date_key >= cutoff_7d {
                    add_to_totals(&mut last_7d, inp, cap, outp, think);
                }
                if date_key >= cutoff_30d {
                    add_to_totals(&mut last_30d, inp, cap, outp, think);
                }
                if source_name == "desktop" {
                    desktop_total += tot;
                } else {
                    cli_total += tot;
                }

                let day = daily_map.entry(date_key).or_default();
                if source_name == "desktop" {
                    day.desktop_total += tot;
                } else {
                    day.cli_total += tot;
                }
                day.input += inp;
                day.cache_read += cap;
                day.output += outp;
                day.thinking += think;
                day.requests += 1;

                let m_acc = model_map.entry(model_display).or_default();
                m_acc.input += inp;
                m_acc.cache_read += cap;
                m_acc.output += outp;
                m_acc.requests += 1;
            }
        }
    }

    calc_totals(&mut today);
    calc_totals(&mut last_7d);
    calc_totals(&mut last_30d);
    calc_totals(&mut all_time);

    // Emit a contiguous 30-day series ending today for clean bar rendering.
    let mut daily = Vec::with_capacity(30);
    for i in (0..30).rev() {
        let d = (now_local - Duration::days(i)).format("%Y-%m-%d").to_string();
        if let Some(acc) = daily_map.get(&d) {
            daily.push(DailyUsageRow {
                date: d,
                desktop_total: acc.desktop_total,
                cli_total: acc.cli_total,
                input: acc.input,
                cache_read: acc.cache_read,
                output: acc.output,
                total: acc.input + acc.cache_read + acc.output,
                requests: acc.requests,
            });
        } else {
            daily.push(DailyUsageRow {
                date: d,
                desktop_total: 0,
                cli_total: 0,
                input: 0,
                cache_read: 0,
                output: 0,
                total: 0,
                requests: 0,
            });
        }
    }

    let mut by_model: Vec<ModelUsageRow> = model_map
        .into_iter()
        .map(|(model, acc)| ModelUsageRow {
            model,
            input: acc.input,
            cache_read: acc.cache_read,
            output: acc.output,
            total: acc.input + acc.cache_read + acc.output,
            requests: acc.requests,
        })
        .collect();
    by_model.sort_by(|a, b| b.total.cmp(&a.total));

    UsageSummary {
        today,
        last_7d,
        last_30d,
        all_time,
        desktop_total,
        cli_total,
        daily,
        by_model,
        scanned_at_ms: chrono::Utc::now().timestamp_millis(),
    }
}

fn collect_transcript_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), "steps" | "scratch" | "daemon" | "conversations" | "node_modules") {
            continue;
        }
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            collect_transcript_files(&path, depth + 1, out);
        } else if ft.is_file() && name == "transcript.jsonl" {
            out.push(path);
        }
    }
}

fn extract_brain_cascade_id(path: &Path) -> Option<String> {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    for w in parts.windows(2) {
        if w[0].eq_ignore_ascii_case("brain") && w[1].len() == 36 {
            return Some(w[1].to_lowercase());
        }
    }
    None
}

/// Strictly read-only SQLite open that never creates `-wal`/`-shm` or mutates live Antigravity DBs.
fn load_step_models_readonly(db_path: &Path) -> HashMap<i64, String> {
    let mut map = HashMap::new();
    let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
    let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));
    let has_wal = fs::metadata(&wal_path).map(|m| m.len() > 0).unwrap_or(false);
    let has_shm = shm_path.exists();

    let encoded = db_path.to_string_lossy().replace('\\', "/");
    let uri = if !has_wal {
        format!("file:{encoded}?immutable=1")
    } else if has_shm {
        format!("file:{encoded}?mode=ro")
    } else {
        return map;
    };

    let Ok(conn) = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) else {
        return map;
    };
    let Ok(mut stmt) = conn.prepare("SELECT data FROM gen_metadata") else {
        return map;
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0)) else {
        return map;
    };
    for blob in rows.flatten() {
        for (step, model) in extract_gen_metadata_models(&blob) {
            map.insert(step, model);
        }
    }
    map
}

fn normalize_model_label(raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return "Unknown".into();
    }
    let lower = s.to_lowercase();
    if lower.contains("hydrogen") {
        "Gemini 4 Hydrogen".into()
    } else if lower.contains("argon") || lower.contains("orchestrator") {
        "Gemini 4 Argon".into()
    } else if lower.contains("carbon") {
        "Gemini 4 Carbon".into()
    } else if lower.contains("neon") {
        "Gemini 4 Neon".into()
    } else if lower.contains("flash") {
        "Gemini Flash".into()
    } else if lower.contains("opus") {
        "Claude Opus".into()
    } else if lower.contains("sonnet") {
        "Claude Sonnet".into()
    } else if lower.contains("gpt-oss") {
        "GPT-OSS 120B".into()
    } else {
        s.to_string()
    }
}
