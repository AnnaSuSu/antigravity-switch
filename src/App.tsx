import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  Activity,
  ArrowLeft,
  BarChart3,
  Gauge,
  Pin,
  Play,
  Power,
  RefreshCw,
  RotateCw,
  Settings,
  Users,
  X,
} from "lucide-react";
import { api, DashboardOverview } from "./lib/api";
import { AccountsTab } from "./components/AccountsTab";
import { QuotaTab } from "./components/QuotaTab";
import { SessionTab } from "./components/SessionTab";
import { UsageTab } from "./components/UsageTab";
import { SettingsView } from "./components/SettingsView";

type TabKey = "accounts" | "quota" | "session" | "usage";

export default function App() {
  const [tab, setTab] = useState<TabKey>("accounts");
  const [inSettings, setInSettings] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [overview, setOverview] = useState<DashboardOverview | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [toast, setToast] = useState<{ msg: string; ok: boolean } | null>(null);
  const shellRef = useRef<HTMLDivElement | null>(null);
  const lastHeightRef = useRef<number>(0);

  const notify = (msg: string, ok = true) => {
    setToast({ msg, ok });
  };

  const loadOverview = async () => {
    try {
      const data = await api.getOverview();
      setOverview(data);
    } catch (e) {
      notify(String(e), false);
    }
  };

  const handleRefreshAll = async () => {
    setRefreshing(true);
    try {
      const data = await api.refreshAllQuotas();
      setOverview(data);
      notify("配额与状态已刷新");
    } catch (e) {
      notify(String(e), false);
    } finally {
      setRefreshing(false);
    }
  };

  const handleTogglePin = async () => {
    const next = !pinned;
    setPinned(next);
    await api.setWindowPinned(next);
  };

  const handleQuickLaunch = async () => {
    setLaunching(true);
    try {
      const restart = Boolean(overview?.daemonOnline);
      const msg = await api.launchAntigravity(restart);
      notify(msg, true);
    } catch (e) {
      notify(String(e), false);
    } finally {
      setLaunching(false);
    }
  };

  useEffect(() => {
    const el = shellRef.current;
    if (!el) return;
    const syncHeight = () => {
      const h = el.getBoundingClientRect().height;
      if (h > 0 && Math.abs(h - lastHeightRef.current) >= 0.25) {
        lastHeightRef.current = h;
        void api.setPopoverHeight(h);
      }
    };
    syncHeight();
    const ro = new ResizeObserver(syncHeight);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    void loadOverview();
    const unlisten = listen("popover-shown", () => {
      void loadOverview();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 3500);
    return () => clearTimeout(t);
  }, [toast]);

  const activeShort = overview?.activeEmail?.split("@")[0] ?? null;
  const isRunning = Boolean(overview?.daemonOnline);
  const proxyOn = Boolean(overview?.proxy.enabled);
  const launchLabel = isRunning
    ? proxyOn
      ? "带代理重启 Antigravity"
      : "重启 Antigravity"
    : proxyOn
      ? "带代理启动 Antigravity"
      : "启动 Antigravity";

  return (
    <div className="popover-shell" ref={shellRef}>
      <header className="header" data-tauri-drag-region>
        {inSettings ? (
          <div className="header-left">
            <button
              className="icon-btn"
              title="返回"
              onClick={() => setInSettings(false)}
            >
              <ArrowLeft size={14} />
            </button>
            <span className="header-title">设置</span>
          </div>
        ) : (
          <div className="header-left" data-tauri-drag-region>
            <span
              className={`status-dot ${isRunning ? "online" : "offline"}`}
              title={isRunning ? "Antigravity 运行中" : "Antigravity 未运行"}
            />
            <span className="header-title" data-tauri-drag-region>
              Antigravity Switch
            </span>
            {tab !== "accounts" && activeShort && (
              <span className="badge" title={overview?.activeEmail ?? undefined}>
                {activeShort}
              </span>
            )}
          </div>
        )}

        <div className="header-actions">
          <button
            className="icon-btn"
            title="刷新全部配额"
            disabled={refreshing}
            onClick={handleRefreshAll}
          >
            <RefreshCw size={13} className={refreshing ? "spin" : ""} />
          </button>
          <button
            className={`icon-btn ${pinned ? "active" : ""}`}
            title={pinned ? "取消钉住（点击外部自动收起）" : "钉住窗口（失焦不自动收起）"}
            onClick={handleTogglePin}
          >
            <Pin size={13} />
          </button>
          <button
            className={`icon-btn ${inSettings ? "active" : ""}`}
            title="设置"
            onClick={() => setInSettings((v) => !v)}
          >
            <Settings size={13} />
          </button>
        </div>
      </header>

      {!inSettings && (
        <div className="seg-wrap">
          <nav className="seg-bar">
            <button
              className={`seg-btn ${tab === "accounts" ? "active" : ""}`}
              onClick={() => setTab("accounts")}
            >
              <Users size={12} />
              账号
            </button>
            <button
              className={`seg-btn ${tab === "quota" ? "active" : ""}`}
              onClick={() => setTab("quota")}
            >
              <Gauge size={12} />
              额度
            </button>
            <button
              className={`seg-btn ${tab === "session" ? "active" : ""}`}
              onClick={() => setTab("session")}
            >
              <Activity size={12} />
              会话
            </button>
            <button
              className={`seg-btn ${tab === "usage" ? "active" : ""}`}
              onClick={() => setTab("usage")}
            >
              <BarChart3 size={12} />
              用量
            </button>
          </nav>
        </div>
      )}

      {toast && (
        <div className={`toast ${toast.ok ? "ok" : "err"}`}>
          <span>{toast.msg}</span>
          <button
            className="icon-btn"
            style={{ width: 18, height: 18 }}
            onClick={() => setToast(null)}
          >
            <X size={11} />
          </button>
        </div>
      )}

      <main className="body">
        {!overview ? (
          <div className="section empty">加载中…</div>
        ) : inSettings ? (
          <SettingsView overview={overview} onUpdate={setOverview} onNotify={notify} />
        ) : tab === "accounts" ? (
          <AccountsTab
            overview={overview}
            busy={refreshing}
            onUpdate={setOverview}
            onNotify={notify}
            onOpenSettings={() => setInSettings(true)}
          />
        ) : tab === "quota" ? (
          <QuotaTab quota={overview.activeQuota} daemonOnline={overview.daemonOnline} />
        ) : tab === "session" ? (
          <SessionTab />
        ) : (
          <UsageTab />
        )}
      </main>

      <footer className="footer">
        <button
          className="footer-btn"
          disabled={launching || !overview?.antigravityAppPath}
          onClick={handleQuickLaunch}
          title={proxyOn ? `通过 ${overview?.proxy.proxyUrl} 启动` : launchLabel}
        >
          {isRunning ? <RotateCw size={12} /> : <Play size={12} />}
          {launchLabel}
        </button>
        <button className="footer-btn danger" onClick={() => void api.quitApp()}>
          <Power size={12} />
          退出
        </button>
      </footer>
    </div>
  );
}
