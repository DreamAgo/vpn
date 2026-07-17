import { useCallback, useEffect, useRef, useState } from "react";
import {
  ConnState,
  DiagnosticsInfo,
  StatusResponse,
  changePassword,
  checkForUpdate,
  connect,
  disconnect,
  getLaunchOnStartup,
  getDiagnosticsInfo,
  getStatus,
  hideWindow,
  installPendingUpdate,
  isLoggedIn,
  login,
  logout,
  notify,
  quitApp,
  savedServer,
  savedUsername,
  setLaunchOnStartup,
  syncTrayState,
} from "./api";
import { formatBytes, formatDuration } from "./format";

const POLL_MS = 2500;
const BACKEND_FAILURE_THRESHOLD = 2;

const STATE_META: Record<ConnState, { label: string; detail: string }> = {
  connected: { label: "已连接", detail: "流量正在通过安全链路转发" },
  connecting: { label: "连接中", detail: "正在建立安全隧道" },
  reconnecting: { label: "重连中", detail: "网络波动,隧道正在恢复" },
  disconnected: { label: "未连接", detail: "当前没有活动安全链路" },
  error: { label: "连接异常", detail: "需要处理后重新连接" },
};

type SettingsTab = "connection" | "updates" | "account";
type ActivityEntry = {
  id: number;
  time: string;
  title: string;
  detail?: string;
};

async function copyText(value: string | null | undefined): Promise<void> {
  const text = value?.trim();
  if (!text || text === "--") return;
  await navigator.clipboard?.writeText(text).catch(() => {});
}

function formatVersion(version: string | null | undefined): string | null {
  const value = version?.trim();
  if (!value) return null;
  return value.startsWith("v") ? value : `v${value}`;
}

export default function App() {
  const [loggedIn, setLoggedIn] = useState<boolean | null>(null);
  const [status, setStatus] = useState<StatusResponse | null>(null);
  const [server, setServer] = useState<string>("");
  const [currentUser, setCurrentUser] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [tick, setTick] = useState(0);
  const [showSettings, setShowSettings] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("connection");
  const [showChangePwd, setShowChangePwd] = useState(false);
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [updateBody, setUpdateBody] = useState<string | null>(null);
  const [updateBusy, setUpdateBusy] = useState(false);
  const [updateMessage, setUpdateMessage] = useState<string | null>(null);
  const [updateProgress, setUpdateProgress] = useState<string | null>(null);
  const [launchOnStartup, setLaunchOnStartupState] = useState(false);
  const [startupBusy, setStartupBusy] = useState(false);
  const [lastError, setLastError] = useState<string | null>(null);
  const [diagnostics, setDiagnostics] = useState<DiagnosticsInfo | null>(null);
  const [backendUnavailable, setBackendUnavailable] = useState(false);
  const [activity, setActivity] = useState<ActivityEntry[]>([]);
  const [toast, setToast] = useState<string | null>(null);
  const pollRef = useRef<number | null>(null);
  const prevStateRef = useRef<ConnState | null>(null);
  const toastTimerRef = useRef<number | null>(null);
  const backendFailuresRef = useRef(0);
  const refreshIdRef = useRef(0);
  const sessionEndingRef = useRef(false);

  const addActivity = useCallback((title: string, detail?: string) => {
    setActivity((items) => [
      {
        id: Date.now(),
        time: new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
        title,
        detail,
      },
      ...items,
    ].slice(0, 5));
  }, []);

  const refresh = useCallback(async () => {
    if (sessionEndingRef.current) return;
    const requestId = ++refreshIdRef.current;
    try {
      const [li, saved, username] = await Promise.all([
        isLoggedIn(),
        savedServer(),
        savedUsername().catch(() => null),
      ]);
      if (sessionEndingRef.current || requestId !== refreshIdRef.current) return;
      setServer(saved ?? "");
      if (!li) {
        backendFailuresRef.current = 0;
        setBackendUnavailable(false);
        void syncTrayState("disconnected");
        setCurrentUser(null);
        setStatus(null);
        setLoggedIn(false);
        return;
      }
      setCurrentUser(username);

      const s = await getStatus();
      if (sessionEndingRef.current || requestId !== refreshIdRef.current) return;
      backendFailuresRef.current = 0;
      setBackendUnavailable(false);
      const prev = prevStateRef.current;
      void syncTrayState(s.state);

      if (s.state === "error" && (s.last_error ?? "").includes("强制下线")) {
        sessionEndingRef.current = true;
        ++refreshIdRef.current;
        prevStateRef.current = null;
        void syncTrayState("disconnected");
        setCurrentUser(null);
        setStatus(null);
        await logout().catch(() => {});
        setLoggedIn(false);
        void notify("已被强制下线", "管理员已将你强制下线,请重新登录后再连接。");
        return;
      }

      if (prev && prev !== s.state) {
        if (s.state === "reconnecting") {
          addActivity("网络波动", "正在自动重连");
          void notify("连接中断", "网络异常,正在自动重连。");
        } else if (s.state === "connected" && prev === "reconnecting") {
          setLastError(null);
          addActivity("已重新连接", "安全链路已恢复");
          void notify("已重新连接", "安全链路已恢复。");
        } else if (s.state === "error") {
          const detail = s.last_error ?? "连接已断开。";
          setLastError(detail);
          addActivity("连接异常", detail);
          void notify("连接出错", detail);
        } else if (s.state === "connected") {
          setLastError(null);
          addActivity("已连接");
        } else if (s.state === "disconnected") {
          addActivity("已断开连接");
        }
      }
      prevStateRef.current = s.state;

      setStatus(s);
      setLoggedIn(true);
    } catch {
      // 旧状态仅保留作诊断展示，不能继续把绿色 Connected 当作实时可信状态。
      if (sessionEndingRef.current || requestId !== refreshIdRef.current) return;
      backendFailuresRef.current += 1;
      if (backendFailuresRef.current >= BACKEND_FAILURE_THRESHOLD) {
        setBackendUnavailable(true);
      }
    }
  }, [addActivity]);

  useEffect(() => {
    refresh();
    pollRef.current = window.setInterval(refresh, POLL_MS);
    const timer = window.setInterval(() => setTick((x) => x + 1), 1000);
    return () => {
      if (pollRef.current) window.clearInterval(pollRef.current);
      window.clearInterval(timer);
    };
  }, [refresh]);

  useEffect(() => {
    return () => {
      if (toastTimerRef.current) window.clearTimeout(toastTimerRef.current);
    };
  }, []);

  useEffect(() => {
    getLaunchOnStartup()
      .then(setLaunchOnStartupState)
      .catch(() => setLaunchOnStartupState(false));
  }, []);

  useEffect(() => {
    getDiagnosticsInfo().then(setDiagnostics).catch(() => setDiagnostics(null));
  }, []);

  const checkUpdates = useCallback(async (manual = false) => {
    setUpdateBusy(true);
    setUpdateMessage(null);
    setUpdateProgress(null);
    try {
      const result = await checkForUpdate();
      if (result.available && result.version) {
        setUpdateVersion(result.version);
        setUpdateBody(result.body);
        setUpdateMessage(`发现新版本 ${result.version}`);
        if (!manual) void notify("发现新版本", `易链 ${result.version} 可更新。`);
      } else {
        setUpdateVersion(null);
        setUpdateBody(null);
        setUpdateMessage("当前已是最新版本");
        if (manual) void notify("已是最新版本", "易链当前无需更新。");
      }
    } catch (e) {
      setUpdateMessage(`检查更新失败: ${String(e)}`);
    } finally {
      setUpdateBusy(false);
    }
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void checkUpdates(false);
    }, 3500);
    return () => window.clearTimeout(timer);
  }, [checkUpdates]);

  void tick;

  const onConnect = async () => {
    setBusy(true);
    addActivity("开始连接");
    try {
      await connect();
      setLastError(null);
    } catch (e) {
      const detail = String(e);
      setLastError(detail);
      addActivity("连接失败", detail);
      void notify("连接失败", detail);
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const onDisconnect = async () => {
    setBusy(true);
    addActivity("正在断开连接");
    try {
      await disconnect();
      addActivity("已断开连接");
    } catch (e) {
      const detail = String(e);
      setLastError(detail);
      addActivity("断开失败", detail);
      void notify("断开失败", detail);
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const onLogout = async () => {
    setBusy(true);
    sessionEndingRef.current = true;
    ++refreshIdRef.current;
    setCurrentUser(null);
    try {
      await logout();
      addActivity("已登出账号");
      setShowSettings(false);
      setLoggedIn(false);
      setStatus(null);
    } catch (e) {
      sessionEndingRef.current = false;
      void refresh();
      void notify("登出失败", String(e));
    } finally {
      setBusy(false);
    }
  };

  const onToggleLaunchOnStartup = async (enabled: boolean) => {
    setStartupBusy(true);
    try {
      await setLaunchOnStartup(enabled);
      setLaunchOnStartupState(enabled);
      addActivity(enabled ? "已启用开机自启" : "已关闭开机自启");
    } catch (e) {
      const detail = String(e);
      setLastError(detail);
      addActivity("开机自启设置失败", detail);
      void notify("开机自启设置失败", detail);
    } finally {
      setStartupBusy(false);
    }
  };

  const onQuit = () => {
    const currentState = status?.state ?? "disconnected";
    const hasActiveLink =
      currentState === "connected" ||
      currentState === "connecting" ||
      currentState === "reconnecting";
    if (hasActiveLink || busy) {
      const ok = window.confirm("当前安全链路可能仍在运行。退出易链会停止托盘进程，确定退出吗？");
      if (!ok) return;
    }
    void quitApp();
  };

  const onCopy = (value: string | null | undefined, label: string) => {
    void copyText(value).then(() => {
      setToast(`已复制${label}`);
      if (toastTimerRef.current) window.clearTimeout(toastTimerRef.current);
      toastTimerRef.current = window.setTimeout(() => setToast(null), 1600);
    });
    addActivity(`已复制${label}`);
  };

  const onInstallUpdate = async () => {
    setUpdateBusy(true);
    setUpdateMessage("正在下载更新...");
    try {
      await installPendingUpdate(({ downloaded, total }) => {
        if (total && total > 0) {
          setUpdateProgress(`${Math.round((downloaded / total) * 100)}%`);
        } else {
          setUpdateProgress(formatBytes(downloaded));
        }
      });
    } catch (e) {
      setUpdateMessage(`安装更新失败: ${String(e)}`);
      setUpdateBusy(false);
    }
  };

  if (loggedIn === null) {
    return <BootView />;
  }

  if (!loggedIn) {
    return (
      <LoginView
        appVersion={diagnostics?.app_version ?? null}
        onLoggedIn={() => {
          sessionEndingRef.current = false;
          setCurrentUser(null);
          setLoggedIn(true);
          refresh();
        }}
      />
    );
  }

  const actualState: ConnState = status?.state ?? "disconnected";
  const state: ConnState = backendUnavailable ? "error" : actualState;
  const meta = STATE_META[state];
  const connected = actualState === "connected";
  const reconnecting = actualState === "reconnecting";
  const tunnelUp = connected || reconnecting;
  const connecting = actualState === "connecting";

  return (
    <div className="app-shell" data-state={state} data-tauri-drag-region>
      <header className="appbar" data-tauri-drag-region>
        <Brand version={diagnostics?.app_version ?? null} />
        <div className="top-actions" data-tauri-drag-region="false">
          <IconButton
            title="设置"
            onClick={() => setShowSettings(true)}
            active={showSettings}
          >
            <SettingsIcon />
          </IconButton>
          <WindowControls />
        </div>
      </header>

      <main className="content">
        <section className="status-card">
          <div className="status-head">
            <StatusGlyph state={state} />
            <div className="status-copy">
              <div className="status-kicker">链路状态</div>
              <div className="status-title">{meta.label}</div>
              <div className="status-detail">
                {backendUnavailable
                  ? "客户端后端无响应，显示状态可能已过期"
                  : state === "error" && status?.last_error
                    ? status.last_error
                    : meta.detail}
              </div>
              <CurrentAccount username={currentUser} />
            </div>
          </div>

          <button
            className={`primary-action ${tunnelUp ? "disconnect" : "connect"}`}
            disabled={busy || connecting}
            onClick={tunnelUp ? onDisconnect : onConnect}
          >
            {busy || connecting ? <span className="spinner" /> : tunnelUp ? "断开连接" : "建立安全链路"}
          </button>
        </section>

        {reconnecting && <div className="notice">网络异常,正在自动恢复连接。</div>}
        {backendUnavailable && <div className="notice">无法读取实时状态，请重新打开客户端并查看本地日志。</div>}

        <section className="metric-grid">
          <Metric label="专属地址" value={status?.vpn_ip ?? "--"} onCopy={() => onCopy(status?.vpn_ip, "专属地址")} />
          <Metric label="连接时长" value={status?.since ? formatDuration(status.since) : "--"} />
          <Metric label="接收" value={formatBytes(status?.bytes_rx ?? 0)} />
          <Metric label="发送" value={formatBytes(status?.bytes_tx ?? 0)} />
        </section>

        <section className="endpoint-card">
          <div>
            <div className="small-label">服务端</div>
            <button className="copy-value endpoint-value" onClick={() => onCopy(server, "服务端地址")}>
              {server || "--"}
            </button>
          </div>
          <button
            className="text-button"
            onClick={() => {
              setSettingsTab("connection");
              setShowSettings(true);
            }}
          >
            详情
          </button>
        </section>
      </main>

      {showSettings && (
        <SettingsPanel
          tab={settingsTab}
          onTabChange={setSettingsTab}
          onClose={() => setShowSettings(false)}
          status={status}
          server={server}
          currentUser={currentUser}
          busy={busy}
          updateBusy={updateBusy}
          updateVersion={updateVersion}
          updateBody={updateBody}
          updateMessage={updateMessage}
          updateProgress={updateProgress}
          launchOnStartup={launchOnStartup}
          startupBusy={startupBusy}
          lastError={lastError}
          activity={activity}
          diagnostics={diagnostics}
          backendUnavailable={backendUnavailable}
          onRefresh={refresh}
          onCheckUpdates={() => checkUpdates(true)}
          onInstallUpdate={onInstallUpdate}
          onToggleLaunchOnStartup={onToggleLaunchOnStartup}
          onChangePassword={() => {
            setShowSettings(false);
            setShowChangePwd(true);
          }}
          onLogout={onLogout}
          onQuit={onQuit}
        />
      )}

      {showChangePwd && (
        <ChangePasswordSheet
          onClose={() => setShowChangePwd(false)}
          onChanged={async () => {
            sessionEndingRef.current = true;
            ++refreshIdRef.current;
            setShowChangePwd(false);
            setCurrentUser(null);
            setStatus(null);
            await logout().catch(() => {});
            setLoggedIn(false);
          }}
        />
      )}

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function Brand({ version }: { version?: string | null }) {
  const displayVersion = formatVersion(version);
  return (
    <div className="brand" data-tauri-drag-region>
      <div className="brand-mark" data-tauri-drag-region>易</div>
      <div className="brand-copy" data-tauri-drag-region>
        <div className="brand-title" data-tauri-drag-region>
          <div className="brand-name" data-tauri-drag-region>易链</div>
          {displayVersion && <span className="brand-version" title={displayVersion} data-tauri-drag-region>{displayVersion}</span>}
        </div>
        <div className="brand-sub" data-tauri-drag-region>安全接入中枢</div>
      </div>
    </div>
  );
}

function BootView() {
  return (
    <div className="app-shell boot" data-state="disconnected" data-tauri-drag-region>
      <div className="boot-controls" data-tauri-drag-region="false">
        <WindowControls />
      </div>
      <div className="boot-card">
        <div className="brand-mark">易</div>
        <div className="boot-text">正在启动...</div>
      </div>
    </div>
  );
}

function WindowControls() {
  const hideToTray = () => {
    void hideWindow();
  };

  return (
    <div className="window-controls" data-tauri-drag-region="false">
      <IconButton title="最小化到托盘" onClick={hideToTray}>
        <MinimizeIcon />
      </IconButton>
      <IconButton title="关闭到托盘" onClick={hideToTray}>
        <CloseIcon />
      </IconButton>
    </div>
  );
}

function StatusGlyph({ state }: { state: ConnState }) {
  return (
    <div className="status-glyph" data-state={state} aria-hidden="true">
      <span className="glyph-ring" />
      <span className="glyph-core" />
      <span className="glyph-check" />
    </div>
  );
}

function Metric({ label, value, onCopy }: { label: string; value: string; onCopy?: () => void }) {
  return (
    <div className="metric">
      <span className="small-label">{label}</span>
      {onCopy ? (
        <button className="copy-value metric-value" onClick={onCopy}>
          {value}
        </button>
      ) : (
        <span className="metric-value">{value}</span>
      )}
    </div>
  );
}

function InfoRow({ label, value, onCopy }: { label: string; value: string; onCopy?: () => void }) {
  return (
    <div className="info-row">
      <span>{label}</span>
      {onCopy ? (
        <button className="copy-value info-value" onClick={onCopy}>
          {value}
        </button>
      ) : (
        <strong>{value}</strong>
      )}
    </div>
  );
}

function CurrentAccount({
  username,
  card = false,
}: {
  username: string | null;
  card?: boolean;
}) {
  const displayUsername = username?.trim() || "未知账号";
  return (
    <div className={`current-account${card ? " account-card" : ""}`}>
      <span className="current-account-label">当前账号</span>
      <strong className="current-account-value" title={displayUsername}>
        {displayUsername}
      </strong>
    </div>
  );
}

function SettingsPanel({
  tab,
  onTabChange,
  onClose,
  status,
  server,
  currentUser,
  busy,
  updateBusy,
  updateVersion,
  updateBody,
  updateMessage,
  updateProgress,
  launchOnStartup,
  startupBusy,
  lastError,
  activity,
  diagnostics,
  backendUnavailable,
  onRefresh,
  onCheckUpdates,
  onInstallUpdate,
  onToggleLaunchOnStartup,
  onChangePassword,
  onLogout,
  onQuit,
}: {
  tab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
  onClose: () => void;
  status: StatusResponse | null;
  server: string;
  currentUser: string | null;
  busy: boolean;
  updateBusy: boolean;
  updateVersion: string | null;
  updateBody: string | null;
  updateMessage: string | null;
  updateProgress: string | null;
  launchOnStartup: boolean;
  startupBusy: boolean;
  lastError: string | null;
  activity: ActivityEntry[];
  diagnostics: DiagnosticsInfo | null;
  backendUnavailable: boolean;
  onRefresh: () => void;
  onCheckUpdates: () => void;
  onInstallUpdate: () => void;
  onToggleLaunchOnStartup: (enabled: boolean) => void;
  onChangePassword: () => void;
  onLogout: () => void;
  onQuit: () => void;
}) {
  const state = status?.state ?? "disconnected";
  const stateLabel = STATE_META[state].label;
  const copyDiagnostics = async () => {
    const currentDiagnostics = await getDiagnosticsInfo().catch(() => diagnostics);
    const diagnostic = [
      `产品: 易链`,
      `状态: ${stateLabel}`,
      `服务端: ${server || "--"}`,
      `专属地址: ${status?.vpn_ip ?? "--"}`,
      `最后错误: ${lastError ?? status?.last_error ?? "--"}`,
      `后端状态: ${backendUnavailable ? "不可用（状态可能过期）" : "正常"}`,
      `客户端版本: ${currentDiagnostics?.app_version ?? "--"}`,
      `平台: ${currentDiagnostics ? `${currentDiagnostics.os}/${currentDiagnostics.arch}` : "--"}`,
      `日志目录: ${currentDiagnostics?.log_dir ?? "--"}`,
      `日志状态: ${currentDiagnostics ? (currentDiagnostics.log_error ?? "正常") : "未知（无法读取诊断信息）"}`,
      `时间: ${new Date().toISOString()}`,
    ].join("\n");
    await navigator.clipboard?.writeText(diagnostic).catch(() => {});
  };

  return (
    <>
      <div className="scrim" onClick={onClose} />
      <aside className="settings-panel" role="dialog" aria-label="设置">
        <div className="panel-head">
          <div>
            <div className="panel-title">设置</div>
            <div className="panel-sub">连接、更新与账号</div>
          </div>
          <IconButton title="关闭" onClick={onClose}>
            <CloseIcon />
          </IconButton>
        </div>

        <div className="tabs" role="tablist">
          <button className={tab === "connection" ? "active" : ""} onClick={() => onTabChange("connection")}>
            连接
          </button>
          <button className={tab === "updates" ? "active" : ""} onClick={() => onTabChange("updates")}>
            更新
          </button>
          <button className={tab === "account" ? "active" : ""} onClick={() => onTabChange("account")}>
            账号
          </button>
        </div>

        <div className="tab-body">
          {tab === "connection" && (
            <div className="stack">
              <div className="inline-card">
                <div className="inline-title">权限提示</div>
                <div className="inline-note">
                  首次建立安全链路需要管理员权限来创建系统隧道。窗口关闭后会保留托盘运行，可从托盘重新打开。
                </div>
              </div>
              <InfoRow label="状态" value={stateLabel} />
              <InfoRow label="服务端" value={server || "--"} onCopy={() => void copyText(server)} />
              <InfoRow label="专属地址" value={status?.vpn_ip ?? "--"} onCopy={() => void copyText(status?.vpn_ip)} />
              <InfoRow label="连接时长" value={status?.since ? formatDuration(status.since) : "--"} />
              {(lastError || status?.last_error) && (
                <div className="inline-card error-card">
                  <div className="inline-title">错误详情</div>
                  <div className="inline-note">{lastError ?? status?.last_error}</div>
                </div>
              )}
              <div className={`inline-card ${diagnostics?.log_error ? "error-card" : ""}`}>
                <div className="inline-title">运行诊断</div>
                <div className="inline-note">
                  {diagnostics?.log_error ?? "可复制客户端版本、平台、日志目录和当前错误。"}
                </div>
                <button className="secondary-action compact" onClick={copyDiagnostics}>
                  复制诊断信息
                </button>
              </div>
              <div className="inline-card">
                <div className="inline-title">连接日志</div>
                {activity.length > 0 ? (
                  <div className="activity-list">
                    {activity.map((item) => (
                      <div className="activity-item" key={item.id}>
                        <span>{item.time}</span>
                        <strong>{item.title}</strong>
                        {item.detail && <em>{item.detail}</em>}
                      </div>
                    ))}
                  </div>
                ) : (
                  <div className="inline-note">暂无连接事件</div>
                )}
              </div>
              <button className="secondary-action" onClick={onRefresh}>
                刷新状态
              </button>
            </div>
          )}

          {tab === "updates" && (
            <div className="stack">
              <InfoRow label="当前版本" value={formatVersion(diagnostics?.app_version) ?? "--"} />
              <div className="update-card">
                <div className="update-title">
                  {updateVersion ? `发现新版本 ${updateVersion}` : updateMessage ?? "自动检查已启用"}
                </div>
                {updateBody && <div className="update-note">{updateBody}</div>}
                {updateProgress && <div className="update-note">下载进度: {updateProgress}</div>}
              </div>
              <button className="secondary-action" disabled={updateBusy} onClick={onCheckUpdates}>
                {updateBusy ? "检查中..." : "检查更新"}
              </button>
              {updateVersion && (
                <button className="primary-action connect compact" disabled={updateBusy} onClick={onInstallUpdate}>
                  {updateBusy ? "更新中..." : "安装并重启"}
                </button>
              )}
            </div>
          )}

          {tab === "account" && (
            <div className="stack">
              <CurrentAccount username={currentUser} card />
              <label className="toggle-row">
                <span>
                  <strong>开机自启</strong>
                  <em>登录系统后自动启动易链并驻留托盘</em>
                </span>
                <input
                  type="checkbox"
                  checked={launchOnStartup}
                  disabled={startupBusy}
                  onChange={(event) => onToggleLaunchOnStartup(event.target.checked)}
                />
              </label>
              <button className="secondary-action" onClick={onChangePassword}>
                修改密码
              </button>
              <button className="secondary-action" disabled={busy} onClick={onLogout}>
                登出账号
              </button>
              <button className="secondary-action danger" onClick={onQuit}>
                退出 App
              </button>
            </div>
          )}
        </div>
      </aside>
    </>
  );
}

function ChangePasswordSheet({
  onClose,
  onChanged,
}: {
  onClose: () => void;
  onChanged: () => void;
}) {
  const [cur, setCur] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const strong = next.length >= 8 && /[a-zA-Z]/.test(next) && /[0-9]/.test(next);
  const valid = cur.length > 0 && strong && next === confirm;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setErr(null);
    if (next !== confirm) {
      setErr("两次输入的新密码不一致");
      return;
    }
    setBusy(true);
    try {
      await changePassword(cur, next);
      void notify("密码已修改", "请用新密码重新登录");
      onChanged();
    } catch (e2) {
      setErr(String(e2));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="scrim" onClick={() => !busy && onClose()} />
      <aside className="settings-panel password-panel" role="dialog" aria-label="修改密码">
        <div className="panel-head">
          <div>
            <div className="panel-title">修改密码</div>
            <div className="panel-sub">更新后需要重新登录</div>
          </div>
          <IconButton title="关闭" onClick={onClose} disabled={busy}>
            <CloseIcon />
          </IconButton>
        </div>
        <form className="form-stack" onSubmit={submit}>
          <Field label="当前密码">
            <input type="password" value={cur} autoFocus onChange={(e) => setCur(e.target.value)} />
          </Field>
          <Field label="新密码">
            <input type="password" value={next} onChange={(e) => setNext(e.target.value)} />
          </Field>
          <Field label="确认新密码">
            <input type="password" value={confirm} onChange={(e) => setConfirm(e.target.value)} />
          </Field>
          {!strong && next.length > 0 && <div className="form-note">至少 8 位,且同时包含字母和数字。</div>}
          {err && <div className="form-error">{err}</div>}
          <button type="submit" className="primary-action connect compact" disabled={busy || !valid}>
            {busy ? "提交中..." : "确认修改"}
          </button>
        </form>
      </aside>
    </>
  );
}

function LoginView({
  appVersion,
  onLoggedIn,
}: {
  appVersion: string | null;
  onLoggedIn: () => void;
}) {
  const [server, setServer] = useState("https://");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    savedServer().then((s) => {
      if (s) setServer(s);
    });
  }, []);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setErr(null);
    setBusy(true);
    try {
      await login(server.trim(), username.trim(), password);
      onLoggedIn();
    } catch (e2) {
      setErr(String(e2));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="app-shell auth-shell" data-state="disconnected" data-tauri-drag-region>
      <header className="appbar" data-tauri-drag-region>
        <Brand version={appVersion} />
        <WindowControls />
      </header>
      <main className="login-content">
        <div className="login-copy">
          <div className="login-title">登录客户端</div>
          <div className="login-sub">连接到你的安全接入控制面</div>
        </div>
        <form className="form-stack" onSubmit={submit}>
          <Field label="服务端地址">
            <input
              type="text"
              value={server}
              placeholder="https://access.example.com"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              onChange={(e) => setServer(e.target.value)}
            />
          </Field>
          <Field label="用户名">
            <input
              type="text"
              value={username}
              placeholder="username"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              onChange={(e) => setUsername(e.target.value)}
            />
          </Field>
          <Field label="密码">
            <input
              type="password"
              value={password}
              placeholder="password"
              onChange={(e) => setPassword(e.target.value)}
            />
          </Field>
          {err && <div className="form-error">{err}</div>}
          <button className="primary-action connect" disabled={busy || !server || !username || !password}>
            {busy ? "登录中..." : "登录"}
          </button>
        </form>
      </main>
    </div>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
    </label>
  );
}

function IconButton({
  title,
  children,
  onClick,
  disabled,
  active,
}: {
  title: string;
  children: React.ReactNode;
  onClick: () => void;
  disabled?: boolean;
  active?: boolean;
}) {
  return (
    <button
      className={`icon-button${active ? " active" : ""}`}
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

function SettingsIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
      <path fill="currentColor" d="M19.4 13.5c.1-.5.1-1 .1-1.5s0-1-.1-1.5l2-1.5-2-3.4-2.4 1a8 8 0 0 0-2.6-1.5L14 2.5h-4l-.4 2.6A8 8 0 0 0 7 6.6l-2.4-1-2 3.4 2 1.5a9 9 0 0 0 0 3l-2 1.5 2 3.4 2.4-1a8 8 0 0 0 2.6 1.5l.4 2.6h4l.4-2.6a8 8 0 0 0 2.6-1.5l2.4 1 2-3.4zM12 15.5A3.5 3.5 0 1 1 12 8a3.5 3.5 0 0 1 0 7.5z" />
    </svg>
  );
}

function CloseIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
      <path fill="currentColor" d="m6.4 5 12.6 12.6-1.4 1.4L5 6.4z" />
      <path fill="currentColor" d="M17.6 5 5 17.6 6.4 19 19 6.4z" />
    </svg>
  );
}

function MinimizeIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
      <path fill="currentColor" d="M5 11h14v2H5z" />
    </svg>
  );
}
