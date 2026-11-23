import { QuotaSnapshot } from "../lib/api";
import { formatCountdown, formatPercent, formatRelativeTime, quotaColorClass } from "../lib/format";

interface Props {
  quota: QuotaSnapshot | null;
  daemonOnline: boolean;
}

export function QuotaTab({ quota, daemonOnline }: Props) {
  if (!quota || quota.pools.length === 0) {
    return (
      <div className="section empty">
        {daemonOnline
          ? "已连接本地 LanguageServer，暂无配额数据，点右上角刷新重试。"
          : "Antigravity 未运行且暂无本地缓存配额，请先在「账号」页导入或登录。"}
      </div>
    );
  }

  return (
    <div className="stack">
      <div className="row-between" style={{ padding: "0 2px" }}>
        <span className="muted">
          {quota.source === "daemon" ? "本地实时" : "CloudCode"} · {formatRelativeTime(quota.fetchedAtMs)}
        </span>
        {quota.planName && <span className="badge">{quota.planName}</span>}
      </div>

      <div className="section">
        {quota.pools.map((pool) => {
          const color = quotaColorClass(pool.remainingFraction);
          const pctWidth =
            pool.remainingFraction != null ? `${Math.round(pool.remainingFraction * 100)}%` : "0%";
          const countdown = formatCountdown(pool.resetAtMs);

          return (
            <div key={pool.poolKey} className="list-item stack" style={{ gap: 6 }}>
              <div className="row-between">
                <span style={{ fontWeight: 500, fontSize: 12.5 }}>{pool.title}</span>
                <div className="row" style={{ gap: 6, flexShrink: 0 }}>
                  {countdown && <span className="muted num">{countdown} 后重置</span>}
                  <span
                    className="num"
                    style={{
                      fontWeight: 600,
                      fontSize: 12,
                      color: `var(--${color})`,
                      minWidth: 34,
                      textAlign: "right",
                    }}
                  >
                    {pool.isExhausted ? "0%" : formatPercent(pool.remainingFraction)}
                  </span>
                </div>
              </div>

              <div className="bar-track">
                <div className={`bar-fill ${color}`} style={{ width: pctWidth }} />
              </div>

              {pool.buckets.length > 0 && (
                <div className="row" style={{ flexWrap: "wrap", gap: 5 }}>
                  {pool.buckets.map((b) => (
                    <span key={b.bucketId} className={`badge num ${quotaColorClass(b.remainingFraction)}`}>
                      {b.windowLabel} {formatPercent(b.remainingFraction)}
                      {b.resetAtMs ? ` · ${formatCountdown(b.resetAtMs)}` : ""}
                    </span>
                  ))}
                </div>
              )}

              {pool.models.length > 0 && (
                <div className="muted" style={{ fontSize: 10.5, lineHeight: 1.4 }}>
                  {pool.models.join(" · ")}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
