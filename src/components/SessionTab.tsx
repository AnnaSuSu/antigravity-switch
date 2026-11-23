import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { api, SessionContextSnapshot, SessionListItem } from "../lib/api";
import { contextColorClass, formatRelativeTime, formatTokens } from "../lib/format";

export function SessionTab() {
  const [sessions, setSessions] = useState<SessionListItem[]>([]);
  const [selectedId, setSelectedId] = useState<string>("");
  const [ctx, setCtx] = useState<SessionContextSnapshot | null>(null);
  const [loading, setLoading] = useState(false);

  const loadSessionsAndContext = async (targetId?: string) => {
    setLoading(true);
    try {
      const list = await api.listSessions();
      setSessions(list);
      const id = targetId || selectedId || list[0]?.cascadeId || "";
      if (id && id !== selectedId) setSelectedId(id);
      const snap = await api.getSessionContext(id || null);
      setCtx(snap);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void loadSessionsAndContext();
  }, []);

  const handleSelect = (id: string) => {
    setSelectedId(id);
    void loadSessionsAndContext(id);
  };

  const pct = ctx?.percent ?? null;
  const color = contextColorClass(pct);
  const speed = ctx?.latestSpeed ?? null;

  return (
    <div className="stack">
      <div className="row" style={{ gap: 5 }}>
        <select
          className="select"
          value={selectedId}
          onChange={(e) => handleSelect(e.target.value)}
          disabled={loading || sessions.length === 0}
        >
          {sessions.length === 0 ? (
            <option value="">暂无本地会话</option>
          ) : (
            sessions.map((s) => (
              <option key={s.cascadeId} value={s.cascadeId}>
                [{s.source === "cli" ? "CLI" : "桌面"}] {s.title} · {formatRelativeTime(s.modifiedAtMs)}
              </option>
            ))
          )}
        </select>
        <button
          className="icon-btn"
          title="刷新会话"
          disabled={loading}
          onClick={() => loadSessionsAndContext(selectedId)}
        >
          <RefreshCw size={12} className={loading ? "spin" : ""} />
        </button>
      </div>

      {ctx?.error && !ctx.usedTokens ? (
        <div className="section empty">{ctx.error}</div>
      ) : ctx ? (
        <>
          <div className="section section-pad stack">
            <div className="row-between">
              <div style={{ minWidth: 0 }}>
                <div style={{ fontWeight: 600, fontSize: 12.5 }}>
                  {ctx.model || "当前会话上下文"}
                </div>
                <div className="muted">
                  {ctx.daemonConnected ? "实时连接" : "离线记录"}
                  {ctx.estimated ? " · 估算" : " · 实测"}
                </div>
              </div>
              <span
                className="num"
                style={{ fontSize: 16, fontWeight: 700, color: `var(--${color})` }}
              >
                {pct != null ? `${pct.toFixed(1)}%` : formatTokens(ctx.usedTokens)}
              </span>
            </div>

            <div className="bar-track">
              <div
                className={`bar-fill ${color}`}
                style={{ width: pct != null ? `${Math.min(100, pct)}%` : "0%" }}
              />
            </div>

            <div className="row-between muted num">
              <span>已用 {formatTokens(ctx.usedTokens)}</span>
              <span>
                {ctx.budgetTokens
                  ? `上限 ${formatTokens(ctx.budgetTokens)} · 剩 ${formatTokens(
                      Math.max(0, ctx.budgetTokens - (ctx.usedTokens || 0))
                    )}`
                  : "上限未知"}
              </span>
            </div>

            <div className="grid-4">
              <div className="metric-cell">
                <div className="metric-lbl">输入</div>
                <div className="metric-val">{formatTokens(ctx.inputTokens)}</div>
              </div>
              <div className="metric-cell">
                <div className="metric-lbl">缓存</div>
                <div className="metric-val">{formatTokens(ctx.cacheReadTokens)}</div>
              </div>
              <div className="metric-cell">
                <div className="metric-lbl">输出</div>
                <div className="metric-val">{formatTokens(ctx.outputTokens)}</div>
              </div>
              <div className="metric-cell">
                <div className="metric-lbl">思考</div>
                <div className="metric-val">{formatTokens(ctx.thinkingTokens)}</div>
              </div>
            </div>
          </div>

          {speed && (
            <div className="section section-pad stack" style={{ gap: 7 }}>
              <div className="row-between">
                <span className="section-label" style={{ padding: 0 }}>
                  最近生成速度
                </span>
                <span className="badge accent num">
                  {speed.streamingTps ?? speed.tps ?? "—"} tok/s
                </span>
              </div>
              <div className="grid-4">
                <div className="metric-cell">
                  <div className="metric-lbl">流式</div>
                  <div className="metric-val">{speed.streamingTps ?? "—"}</div>
                </div>
                <div className="metric-cell">
                  <div className="metric-lbl">TTFT</div>
                  <div className="metric-val">
                    {speed.ttftSec != null ? `${speed.ttftSec}s` : "—"}
                  </div>
                </div>
                <div className="metric-cell">
                  <div className="metric-lbl">总耗时</div>
                  <div className="metric-val">
                    {speed.totalSec != null ? `${speed.totalSec}s` : "—"}
                  </div>
                </div>
                <div className="metric-cell">
                  <div className="metric-lbl">输出量</div>
                  <div className="metric-val">{formatTokens(speed.outputTokens)}</div>
                </div>
              </div>
            </div>
          )}

          {ctx.recentTurns.length > 1 && (
            <div className="section">
              {ctx.recentTurns.slice(0, 5).map((t, i) => (
                <div key={i} className="list-item row-between" style={{ padding: "7px 11px" }}>
                  <span
                    style={{
                      fontSize: 11.5,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {t.model}
                  </span>
                  <span className="muted num" style={{ flexShrink: 0 }}>
                    {formatTokens(t.inputTokens + t.cacheReadTokens)} → {formatTokens(t.outputTokens)}
                    {t.streamingTps != null ? ` · ${t.streamingTps}t/s` : ""}
                  </span>
                </div>
              ))}
            </div>
          )}
        </>
      ) : null}
    </div>
  );
}
