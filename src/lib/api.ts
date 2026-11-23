import { invoke } from "@tauri-apps/api/core";

export interface QuotaBucket {
  bucketId: string;
  windowKey: string;
  windowLabel: string;
  remainingFraction: number | null;
  resetTimeIso: string | null;
  resetAtMs: number | null;
  isExhausted: boolean;
}

export interface QuotaPool {
  poolKey: string;
  title: string;
  models: string[];
  remainingFraction: number | null;
  isExhausted: boolean;
  resetTimeIso: string | null;
  resetAtMs: number | null;
  buckets: QuotaBucket[];
}

export interface QuotaSnapshot {
  email: string | null;
  planName: string | null;
  pools: QuotaPool[];
  primaryFraction: number | null;
  fetchedAtMs: number;
  source: string;
}

export interface AccountSummary {
  email: string;
  name: string;
  isActive: boolean;
  planName: string | null;
  lastQuota: QuotaSnapshot | null;
  lastError: string | null;
  hasRefreshToken: boolean;
  lastUsedAtMs: number;
}

export interface AutoSwitchConfig {
  enabled: boolean;
  poolKey: string;
  windowKey: string;
  thresholdPct: number;
  cooldownSec: number;
  candidates: string[];
}

export interface ProxyConfig {
  enabled: boolean;
  proxyUrl: string;
  bypassList: string;
}

export interface TrayDisplayConfig {
  showQuota: boolean;
  showContext: boolean;
  showSpeed: boolean;
}

export interface SwitchLog {
  atMs: number;
  fromEmail: string | null;
  toEmail: string;
  trigger: string;
  ok: boolean;
  message: string;
}

export interface DashboardOverview {
  daemonOnline: boolean;
  daemonEmail: string | null;
  activeEmail: string | null;
  accounts: AccountSummary[];
  activeQuota: QuotaSnapshot | null;
  autoSwitch: AutoSwitchConfig;
  proxy: ProxyConfig;
  trayDisplay: TrayDisplayConfig;
  switchRestartsApp: boolean;
  antigravityAppPath: string | null;
  lastSwitch: SwitchLog | null;
}

export interface SwitchResult {
  ok: boolean;
  email: string;
  verified: boolean;
  rolledBack: boolean;
  message: string;
}

export interface SessionListItem {
  cascadeId: string;
  title: string;
  modifiedAtMs: number;
  source: string;
}

export interface TurnSpeedMetric {
  model: string;
  inputTokens: number;
  cacheReadTokens: number;
  outputTokens: number;
  thinkingTokens: number;
  ttftSec: number | null;
  streamingSec: number | null;
  totalSec: number | null;
  tps: number | null;
  streamingTps: number | null;
}

export interface SessionContextSnapshot {
  cascadeId: string;
  model: string;
  usedTokens: number | null;
  budgetTokens: number | null;
  percent: number | null;
  inputTokens: number | null;
  cacheReadTokens: number | null;
  outputTokens: number | null;
  thinkingTokens: number | null;
  estimated: boolean;
  latestSpeed: TurnSpeedMetric | null;
  recentTurns: TurnSpeedMetric[];
  daemonConnected: boolean;
  error: string | null;
}

export interface TokenTotals {
  input: number;
  cacheRead: number;
  output: number;
  thinking: number;
  total: number;
  requests: number;
  cacheHitRatio: number;
}

export interface DailyUsageRow {
  date: string;
  desktopTotal: number;
  cliTotal: number;
  input: number;
  cacheRead: number;
  output: number;
  total: number;
  requests: number;
}

export interface ModelUsageRow {
  model: string;
  input: number;
  cacheRead: number;
  output: number;
  total: number;
  requests: number;
}

export interface UsageSummary {
  today: TokenTotals;
  last7d: TokenTotals;
  last30d: TokenTotals;
  allTime: TokenTotals;
  desktopTotal: number;
  cliTotal: number;
  daily: DailyUsageRow[];
  byModel: ModelUsageRow[];
  scannedAtMs: number;
}

export interface ProxyTestResult {
  ok: boolean;
  latencyMs: number | null;
  statusCode: number | null;
  message: string;
}

export const api = {
  getOverview: () => invoke<DashboardOverview>("get_overview"),
  importCurrentAccount: () => invoke<DashboardOverview>("import_current_account"),
  oauthAddAccount: () => invoke<DashboardOverview>("oauth_add_account"),
  cancelOauthLogin: () => invoke<boolean>("cancel_oauth_login_cmd"),
  removeAccount: (email: string) => invoke<DashboardOverview>("remove_account", { email }),
  switchAccount: (email: string) => invoke<SwitchResult>("switch_account_cmd", { email }),
  refreshAllQuotas: () => invoke<DashboardOverview>("refresh_all_quotas"),
  saveAutoSwitchConfig: (config: AutoSwitchConfig) =>
    invoke<DashboardOverview>("save_auto_switch_config", { config }),
  saveTrayDisplayConfig: (config: TrayDisplayConfig) =>
    invoke<DashboardOverview>("save_tray_display_config", { config }),
  saveSwitchBehavior: (restartApp: boolean) =>
    invoke<DashboardOverview>("save_switch_behavior", { restartApp }),
  setWindowPinned: (pinned: boolean) => invoke<boolean>("set_window_pinned", { pinned }),
  setPopoverHeight: (height: number) => invoke<void>("set_popover_height", { height }),
  quitApp: () => invoke<void>("quit_app"),
  listSessions: () => invoke<SessionListItem[]>("list_sessions"),
  getSessionContext: (cascadeId?: string | null) =>
    invoke<SessionContextSnapshot>("get_session_context", { cascadeId: cascadeId || null }),
  getUsageSummary: () => invoke<UsageSummary>("get_usage_summary"),
  saveProxyConfig: (config: ProxyConfig) =>
    invoke<DashboardOverview>("save_proxy_config", { config }),
  testProxy: (proxyUrl: string) => invoke<ProxyTestResult>("test_proxy", { proxyUrl }),
  launchAntigravity: (restartIfRunning: boolean) =>
    invoke<string>("launch_antigravity_cmd", { restartIfRunning }),
};
