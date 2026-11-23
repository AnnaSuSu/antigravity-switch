import { useState } from "react";
import { ArrowRightLeft, Download, Plus, Sparkles, Trash2, X } from "lucide-react";
import { AccountSummary, api, DashboardOverview } from "../lib/api";
import { formatCountdown, formatPercent, quotaColorClass } from "../lib/format";

interface Props {
  overview: DashboardOverview;
  busy: boolean;
  onUpdate: (next: DashboardOverview) => void;
  onNotify: (msg: string, ok?: boolean) => void;
  onOpenSettings: () => void;
}

function tightestPoolSummary(acct: AccountSummary): string | null {
  const pools = acct.lastQuota?.pools;
  if (!pools || pools.length === 0) return null;
  const scored = pools
    .filter((p) => p.remainingFraction != null)
    .sort((a, b) => (a.remainingFraction ?? 1) - (b.remainingFraction ?? 1));
  const tight = scored[0];
  if (!tight) return null;

  const reset = formatCountdown(tight.resetAtMs);
  if ((tight.remainingFraction ?? 1) >= 0.995) {
    return reset ? `全部满额 · ${reset} 后重置` : "全部额度充足";
  }
  const shortTitle = tight.title.split(" / ")[0];
  return reset ? `${shortTitle} · ${reset} 后重置` : shortTitle;
}

export function AccountsTab({ overview, busy, onUpdate, onNotify, onOpenSettings }: Props) {
  const [working, setWorking] = useState<string | null>(null);

  const handleImport = async () => {
    setWorking("import");
    try {
      const next = await api.importCurrentAccount();
      onUpdate(next);
      onNotify("已导入当前本机账号");
    } catch (e) {
      onNotify(String(e), false);
    } finally {
      setWorking(null);
    }
  };

  const handleOAuth = async () => {
    setWorking("oauth");
    try {
      const next = await api.oauthAddAccount();
      onUpdate(next);
      onNotify("Google 账号添加成功");
    } catch (e) {
      const msg = String(e);
      onNotify(msg, msg.includes("已取消"));
    } finally {
      setWorking(null);
    }
  };

  const handleCancelOAuth = async () => {
    try {
      await api.cancelOauthLogin();
    } catch {
      setWorking(null);
    }
  };

  const handleSwitch = async (email: string) => {
    setWorking(`switch:${email}`);
    try {
      const res = await api.switchAccount(email);
      const next = await api.getOverview();
      onUpdate(next);
      onNotify(res.message, res.ok);
    } catch (e) {
      onNotify(String(e), false);
    } finally {
      setWorking(null);
    }
  };

  const handleRemove = async (email: string) => {
    setWorking(`del:${email}`);
    try {
      const next = await api.removeAccount(email);
      onUpdate(next);
      onNotify(`已移除 ${email}`);
    } catch (e) {
      onNotify(String(e), false);
    } finally {
      setWorking(null);
    }
  };

  const isDisabled = busy || working !== null;

  return (
    <div className="stack">
      <div className="row-between" style={{ padding: "0 2px" }}>
        <div className="row">
          <span className="section-label">账号 · {overview.accounts.length}</span>
          {overview.autoSwitch.enabled && (
            <button
              className="badge accent"
              style={{ cursor: "pointer" }}
              title="点击前往设置调整自动切号规则"
              onClick={onOpenSettings}
            >
              <Sparkles size={10} style={{ marginRight: 3 }} />
              自动切 ≤{overview.autoSwitch.thresholdPct}%
            </button>
          )}
        </div>
        <div className="row" style={{ gap: 4 }}>
          <button
            className="btn btn-xs"
            disabled={isDisabled}
            onClick={handleImport}
            title="读取本机 Antigravity 当前登录账号"
          >
            <Download size={11} />
            {working === "import" ? "导入中…" : "导入当前"}
          </button>
          {working === "oauth" ? (
            <button className="btn btn-xs btn-danger" onClick={handleCancelOAuth}>
              <X size={11} />
              取消授权
            </button>
          ) : (
            <button
              className="btn btn-xs"
              disabled={isDisabled}
              onClick={handleOAuth}
              title="通过 Google 浏览器登录添加新账号"
            >
              <Plus size={11} />
              登录添加
            </button>
          )}
        </div>
      </div>

      {overview.accounts.length === 0 ? (
        <div className="section empty">
          暂无账号。点「导入当前」读取本机账号，或「登录添加」添加新账号。
        </div>
      ) : (
        <div className="section">
          {overview.accounts.map((acct) => {
            const frac = acct.lastQuota?.primaryFraction ?? null;
            const color = quotaColorClass(frac);
            const pctWidth = frac != null ? `${Math.round(frac * 100)}%` : "0%";
            const switchingThis = working === `switch:${acct.email}`;
            const subInfo = tightestPoolSummary(acct);

            return (
              <div key={acct.email} className={`list-item ${acct.isActive ? "is-active" : ""}`}>
                <div className="row-between" style={{ marginBottom: 6 }}>
                  <div className="row" style={{ minWidth: 0, flex: 1 }}>
                    <span
                      style={{
                        fontWeight: acct.isActive ? 600 : 500,
                        fontSize: 12.5,
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {acct.email}
                    </span>
                    {acct.planName && <span className="badge">{acct.planName}</span>}
                  </div>
                  <div className="row" style={{ flexShrink: 0, gap: 4 }}>
                    {!acct.isActive && (
                      <button
                        className="icon-btn row-delete"
                        style={{ width: 22, height: 22 }}
                        title="移除账号"
                        disabled={isDisabled}
                        onClick={() => handleRemove(acct.email)}
                      >
                        <Trash2 size={11.5} />
                      </button>
                    )}
                    {acct.isActive ? (
                      <span className="badge good">当前</span>
                    ) : (
                      <button
                        className="btn btn-xs"
                        disabled={isDisabled}
                        onClick={() => handleSwitch(acct.email)}
                      >
                        <ArrowRightLeft size={10.5} />
                        {switchingThis ? "切换中…" : "切换"}
                      </button>
                    )}
                  </div>
                </div>

                <div className="bar-track">
                  <div className={`bar-fill ${color}`} style={{ width: pctWidth }} />
                </div>

                <div className="row-between" style={{ marginTop: 5 }}>
                  <span className="muted" style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                    {acct.lastError ? (
                      <span style={{ color: "var(--danger)" }}>{acct.lastError}</span>
                    ) : (
                      subInfo ?? "未刷新额度"
                    )}
                  </span>
                  <span
                    className="num"
                    style={{
                      fontSize: 11,
                      fontWeight: 600,
                      color: `var(--${color})`,
                      flexShrink: 0,
                    }}
                  >
                    {formatPercent(frac)}
                  </span>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
