export function formatTokens(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(Math.round(n));
}

export function formatPercent(fraction: number | null | undefined): string {
  if (fraction == null || !Number.isFinite(fraction)) return "—";
  const pct = Math.max(0, Math.min(100, fraction * 100));
  if (pct === 0) return "0%";
  if (pct < 1) return "<1%";
  return `${Math.round(pct)}%`;
}

export function formatCountdown(resetAtMs: number | null | undefined): string {
  if (!resetAtMs || !Number.isFinite(resetAtMs)) return "";
  const diff = resetAtMs - Date.now();
  if (diff <= 0) return "即将重置";
  const totalMin = Math.ceil(diff / 60_000);
  const days = Math.floor(totalMin / 1440);
  const hours = Math.floor((totalMin % 1440) / 60);
  const mins = totalMin % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

export function formatRelativeTime(ms: number | null | undefined): string {
  if (!ms) return "";
  const diffSec = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (diffSec < 60) return "刚刚";
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)} 分钟前`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)} 小时前`;
  const d = new Date(ms);
  return `${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes()
  ).padStart(2, "0")}`;
}

export function quotaColorClass(fraction: number | null | undefined): "good" | "warn" | "danger" | "muted" {
  if (fraction == null) return "muted";
  if (fraction <= 0.1) return "danger";
  if (fraction <= 0.3) return "warn";
  return "good";
}

export function contextColorClass(percent: number | null | undefined): "good" | "warn" | "danger" | "muted" {
  if (percent == null) return "muted";
  if (percent >= 85) return "danger";
  if (percent >= 65) return "warn";
  return "good";
}
