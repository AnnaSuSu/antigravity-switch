import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { api, UsageSummary } from "../lib/api";
import { formatTokens } from "../lib/format";

export function UsageTab() {
  const [usage, setUsage] = useState<UsageSummary | null>(null);
  const [range, setRange] = useState<7 | 30>(7);
  const [loading, setLoading] = useState(false);

  const load = async () => {
    setLoading(true);
    try {
      const data = await api.getUsageSummary();
      setUsage(data);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  if (!usage && loading) {
    return <div className="section empty">正在扫描本地 Desktop 与 CLI 用量…</div>;
  }
  if (!usage) {
    return <div className="section empty">暂无本地用量记录</div>;
  }

  const slice = usage.daily.slice(-range);
  const maxDay = Math.max(1, ...slice.map((d) => d.total));
  const topModels = usage.byModel.slice(0, 5);
  const maxModel = Math.max(1, topModels[0]?.total ?? 1);

  return (
    <div className="stack">
      <div className="grid-2">
        <div className="metric-cell">
          <div className="metric-lbl">今日用量</div>
          <div className="metric-val">{formatTokens(usage.today.total)}</div>
          <div className="muted num">{usage.today.requests} 次请求</div>
        </div>
        <div className="metric-cell">
          <div className="metric-lbl">近 7 天</div>
          <div className="metric-val">{formatTokens(usage.last7d.total)}</div>
          <div className="muted num">缓存命中 {usage.last7d.cacheHitRatio}%</div>
        </div>
        <div className="metric-cell">
          <div className="metric-lbl">近 30 天</div>
          <div className="metric-val">{formatTokens(usage.last30d.total)}</div>
          <div className="muted num">{usage.last30d.requests} 次请求</div>
        </div>
        <div className="metric-cell">
          <div className="metric-lbl">累计总量</div>
          <div className="metric-val">{formatTokens(usage.allTime.total)}</div>
          <div className="muted num">
            桌面 {formatTokens(usage.desktopTotal)} / CLI {formatTokens(usage.cliTotal)}
          </div>
        </div>
      </div>

      <div className="section section-pad stack" style={{ gap: 6 }}>
        <div className="row-between">
          <span className="section-label" style={{ padding: 0 }}>
            每日趋势
          </span>
          <div className="row" style={{ gap: 4 }}>
            <button
              className={`btn btn-xs ${range === 7 ? "btn-primary" : ""}`}
              onClick={() => setRange(7)}
            >
              7天
            </button>
            <button
              className={`btn btn-xs ${range === 30 ? "btn-primary" : ""}`}
              onClick={() => setRange(30)}
            >
              30天
            </button>
            <button
              className="icon-btn"
              style={{ width: 22, height: 22 }}
              title="重新扫描"
              disabled={loading}
              onClick={load}
            >
              <RefreshCw size={11} className={loading ? "spin" : ""} />
            </button>
          </div>
        </div>

        <div className="chart-bars">
          {slice.map((day) => {
            const deskPct = (day.desktopTotal / maxDay) * 100;
            const cliPct = (day.cliTotal / maxDay) * 100;
            return (
              <div
                key={day.date}
                className="chart-col"
                title={`${day.date}\n总计: ${formatTokens(day.total)} (${day.requests}次)\n桌面: ${formatTokens(
                  day.desktopTotal
                )} / CLI: ${formatTokens(day.cliTotal)}`}
              >
                {cliPct > 0 && (
                  <div className="chart-seg cli" style={{ height: `${Math.max(3, cliPct)}%` }} />
                )}
                {deskPct > 0 && (
                  <div
                    className="chart-seg desktop"
                    style={{ height: `${Math.max(3, deskPct)}%` }}
                  />
                )}
                {day.total === 0 && (
                  <div className="chart-seg" style={{ height: 2, background: "var(--track)" }} />
                )}
              </div>
            );
          })}
        </div>

        <div className="row-between muted num" style={{ fontSize: 10.5 }}>
          <span>{slice[0]?.date.slice(5)}</span>
          <span className="row" style={{ gap: 8 }}>
            <span>
              <i
                style={{
                  display: "inline-block",
                  width: 6,
                  height: 6,
                  borderRadius: 2,
                  background: "var(--desktop)",
                  marginRight: 4,
                }}
              />
              桌面
            </span>
            <span>
              <i
                style={{
                  display: "inline-block",
                  width: 6,
                  height: 6,
                  borderRadius: 2,
                  background: "var(--cli)",
                  marginRight: 4,
                }}
              />
              CLI
            </span>
          </span>
          <span>{slice[slice.length - 1]?.date.slice(5)}</span>
        </div>
      </div>

      {topModels.length > 0 && (
        <div className="section">
          {topModels.map((m) => (
            <div key={m.model} className="list-item stack" style={{ padding: "8px 11px", gap: 5 }}>
              <div className="row-between" style={{ fontSize: 11.5 }}>
                <span>{m.model}</span>
                <span className="muted num">
                  {formatTokens(m.total)} · {m.requests}次
                </span>
              </div>
              <div className="bar-track">
                <div
                  className="bar-fill accent"
                  style={{ width: `${Math.max(4, (m.total / maxModel) * 100)}%` }}
                />
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
