import { useEffect, useState } from "react";
import { Play, RotateCw, Wifi } from "lucide-react";
import {
  api,
  AutoSwitchConfig,
  DashboardOverview,
  ProxyConfig,
  ProxyTestResult,
  TrayDisplayConfig,
} from "../lib/api";

interface Props {
  overview: DashboardOverview;
  onUpdate: (next: DashboardOverview) => void;
  onNotify: (msg: string, ok?: boolean) => void;
}

export function SettingsView({ overview, onUpdate, onNotify }: Props) {
  const [tray, setTray] = useState<TrayDisplayConfig>(overview.trayDisplay);
  const [rule, setRule] = useState<AutoSwitchConfig>(overview.autoSwitch);
  const [proxy, setProxy] = useState<ProxyConfig>(overview.proxy);
  const [restartApp, setRestartApp] = useState<boolean>(overview.switchRestartsApp);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<ProxyTestResult | null>(null);
  const [saving, setSaving] = useState<string | null>(null);

  useEffect(() => {
    setTray(overview.trayDisplay);
    setRule(overview.autoSwitch);
    setProxy(overview.proxy);
    setRestartApp(overview.switchRestartsApp);
  }, [overview]);

  const toggleTrayItem = async (key: keyof TrayDisplayConfig) => {
    const nextCfg = { ...tray, [key]: !tray[key] };
    setTray(nextCfg);
    try {
      const next = await api.saveTrayDisplayConfig(nextCfg);
      onUpdate(next);
    } catch (e) {
      onNotify(String(e), false);
    }
  };

  const toggleRestartBehavior = async () => {
    const nextVal = !restartApp;
    setRestartApp(nextVal);
    try {
      const next = await api.saveSwitchBehavior(nextVal);
      onUpdate(next);
    } catch (e) {
      onNotify(String(e), false);
    }
  };

  const handleSaveRule = async () => {
    setSaving("rule");
    try {
      const next = await api.saveAutoSwitchConfig(rule);
      onUpdate(next);
      onNotify("自动切号规则已保存");
    } catch (e) {
      onNotify(String(e), false);
    } finally {
      setSaving(null);
    }
  };

  const toggleCandidate = (email: string) => {
    setRule((prev) => {
      const exists = prev.candidates.includes(email);
      return {
        ...prev,
        candidates: exists
          ? prev.candidates.filter((c) => c !== email)
          : [...prev.candidates, email],
      };
    });
  };

  const handleTestProxy = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const res = await api.testProxy(proxy.proxyUrl);
      setTestResult(res);
    } catch (e) {
      setTestResult({ ok: false, latencyMs: null, statusCode: null, message: String(e) });
    } finally {
      setTesting(false);
    }
  };

  const handleSaveProxy = async () => {
    setSaving("proxy");
    try {
      const next = await api.saveProxyConfig(proxy);
      onUpdate(next);
      onNotify("代理配置已保存");
    } catch (e) {
      onNotify(String(e), false);
    } finally {
      setSaving(null);
    }
  };

  const handleLaunch = async (restart: boolean) => {
    setSaving("launch");
    try {
      const saved = await api.saveProxyConfig(proxy);
      onUpdate(saved);
      const msg = await api.launchAntigravity(restart);
      onNotify(msg, true);
    } catch (e) {
      onNotify(String(e), false);
    } finally {
      setSaving(null);
    }
  };

  return (
    <div className="stack">
      {/* 1. Menubar display */}
      <span className="section-label">菜单栏显示</span>
      <div className="section">
        <div className="list-item row-between">
          <span>剩余额度百分比</span>
          <button
            type="button"
            className={`toggle ${tray.showQuota ? "on" : ""}`}
            onClick={() => toggleTrayItem("showQuota")}
          />
        </div>
        <div className="list-item row-between">
          <span>当前会话上下文</span>
          <button
            type="button"
            className={`toggle ${tray.showContext ? "on" : ""}`}
            onClick={() => toggleTrayItem("showContext")}
          />
        </div>
        <div className="list-item row-between">
          <span>生成速度 (tok/s)</span>
          <button
            type="button"
            className={`toggle ${tray.showSpeed ? "on" : ""}`}
            onClick={() => toggleTrayItem("showSpeed")}
          />
        </div>
      </div>

      {/* 2. Auto-Switch */}
      <span className="section-label">阈值自动切号</span>
      <div className="section section-pad stack">
        <div className="row-between">
          <span>低额度时自动切至最高余量账号</span>
          <button
            type="button"
            className={`toggle ${rule.enabled ? "on" : ""}`}
            onClick={() => setRule({ ...rule, enabled: !rule.enabled })}
          />
        </div>

        <div className="grid-2">
          <label className="field">
            监控额度池
            <select
              className="select"
              value={rule.poolKey}
              onChange={(e) => setRule({ ...rule, poolKey: e.target.value })}
            >
              <option value="shared_gemini_main">Gemini 4 Hydrogen / Flash</option>
              <option value="shared_3p_pool">Claude 5.5 / GPT-OSS</option>
              <option value="shared_gemini_argon">Gemini 4 Argon</option>
              <option value="shared_gemini_carbon">Gemini 4 Carbon</option>
            </select>
          </label>
          <label className="field">
            限额窗口
            <select
              className="select"
              value={rule.windowKey}
              onChange={(e) => setRule({ ...rule, windowKey: e.target.value })}
            >
              <option value="primary">主窗口（最紧缩）</option>
              <option value="5h">5 小时窗口</option>
              <option value="7d">7 天窗口</option>
            </select>
          </label>
        </div>

        <div className="grid-2">
          <label className="field">
            剩余阈值 (%) ≤
            <input
              className="input num"
              type="number"
              min={1}
              max={95}
              value={rule.thresholdPct}
              onChange={(e) => setRule({ ...rule, thresholdPct: Number(e.target.value) || 10 })}
            />
          </label>
          <label className="field">
            冷却时间 (秒)
            <input
              className="input num"
              type="number"
              min={30}
              max={3600}
              value={rule.cooldownSec}
              onChange={(e) => setRule({ ...rule, cooldownSec: Number(e.target.value) || 180 })}
            />
          </label>
        </div>

        {overview.accounts.length > 1 && (
          <div className="field">
            <span>候选账号（不选则默认全部）</span>
            <div className="row" style={{ flexWrap: "wrap", gap: 4, marginTop: 2 }}>
              {overview.accounts.map((a) => {
                const selected = rule.candidates.includes(a.email);
                return (
                  <button
                    key={a.email}
                    type="button"
                    className={`btn btn-xs ${selected ? "btn-primary" : ""}`}
                    onClick={() => toggleCandidate(a.email)}
                  >
                    {a.email.split("@")[0]}
                  </button>
                );
              })}
            </div>
          </div>
        )}

        <div className="row-between">
          <span className="muted">仅触及阈值时按需查询候选号</span>
          <button
            className="btn btn-xs btn-primary"
            disabled={saving !== null}
            onClick={handleSaveRule}
          >
            {saving === "rule" ? "保存中…" : "保存规则"}
          </button>
        </div>
      </div>

      {/* 3. No-TUN Proxy */}
      <span className="section-label">免 TUN 本地代理</span>
      <div className="section section-pad stack">
        <div className="row-between">
          <span>启用本地代理</span>
          <button
            type="button"
            className={`toggle ${proxy.enabled ? "on" : ""}`}
            onClick={() => setProxy({ ...proxy, enabled: !proxy.enabled })}
          />
        </div>

        <label className="field">
          代理地址 (http / https / socks5)
          <input
            className="input mono"
            value={proxy.proxyUrl}
            placeholder="http://127.0.0.1:7897"
            onChange={(e) => setProxy({ ...proxy, proxyUrl: e.target.value })}
          />
        </label>

        <label className="field">
          绕过列表 (NO_PROXY)
          <input
            className="input mono"
            value={proxy.bypassList}
            onChange={(e) => setProxy({ ...proxy, bypassList: e.target.value })}
          />
        </label>

        <div className="row-between">
          <button className="btn btn-xs" disabled={testing} onClick={handleTestProxy}>
            <Wifi size={11} />
            {testing ? "测速中…" : "测试连通"}
          </button>
          <button
            className="btn btn-xs btn-primary"
            disabled={saving !== null}
            onClick={handleSaveProxy}
          >
            {saving === "proxy" ? "保存中…" : "保存代理"}
          </button>
        </div>

        {testResult && (
          <div className={`badge ${testResult.ok ? "good" : "danger"}`} style={{ alignSelf: "flex-start" }}>
            {testResult.message}
          </div>
        )}

        <div className="row" style={{ marginTop: 2 }}>
          <button
            className="btn btn-xs"
            style={{ flex: 1 }}
            disabled={saving !== null || !overview.antigravityAppPath}
            onClick={() => handleLaunch(false)}
          >
            <Play size={11} />
            {proxy.enabled ? "带代理启动" : "直连启动"}
          </button>
          <button
            className="btn btn-xs"
            style={{ flex: 1 }}
            disabled={saving !== null || !overview.antigravityAppPath}
            onClick={() => handleLaunch(true)}
          >
            <RotateCw size={11} />
            重启 Antigravity
          </button>
        </div>
      </div>

      {/* 4. Switch behavior */}
      <span className="section-label">切号行为</span>
      <div className="section">
        <div className="list-item row-between">
          <div>
            <div>切号后重启 Antigravity 窗口</div>
            <div className="muted">默认仅重启 language_server，不关闭聊天窗口</div>
          </div>
          <button
            type="button"
            className={`toggle ${restartApp ? "on" : ""}`}
            onClick={toggleRestartBehavior}
          />
        </div>
      </div>
    </div>
  );
}
