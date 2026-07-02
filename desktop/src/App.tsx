import { useCallback, useEffect, useRef, useState } from "react";
import {
  ConnState,
  StatusResponse,
  changePassword,
  connect,
  disconnect,
  getStatus,
  isLoggedIn,
  login,
  logout,
  notify,
  quitApp,
  savedServer,
} from "./api";
import { formatBytes, formatDuration } from "./format";

const POLL_MS = 2500;

// 仪表用语：每个状态一个简短的"信道"措辞 + 副标题。颜色由 CSS 的 [data-state] 驱动。
const STATE_META: Record<ConnState, { word: string; sub: string }> = {
  connected: { word: "SECURED", sub: "信道已加密" },
  connecting: { word: "LINKING", sub: "正在协商握手" },
  reconnecting: { word: "RELINKING", sub: "隧道保持 · 重连中" },
  disconnected: { word: "OFFLINE", sub: "无活动信道" },
  error: { word: "FAULT", sub: "信道已中断" },
};

export default function App() {
  const [loggedIn, setLoggedIn] = useState<boolean | null>(null);
  const [status, setStatus] = useState<StatusResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [tick, setTick] = useState(0); // drives the live duration counter
  const [showSettings, setShowSettings] = useState(false);
  const [showChangePwd, setShowChangePwd] = useState(false);
  const pollRef = useRef<number | null>(null);
  // 上一次的连接状态，用于检测跃迁(只在真正切换时发原生通知，避免刷屏)。
  const prevStateRef = useRef<ConnState | null>(null);

  const refresh = useCallback(async () => {
    try {
      const li = await isLoggedIn();
      if (!li) {
        setLoggedIn(false);
        return;
      }
      const s = await getStatus();
      const prev = prevStateRef.current;

      // 被管理员强制下线:清凭证、回登录页并发原生通知。
      if (s.state === "error" && (s.last_error ?? "").includes("强制下线")) {
        prevStateRef.current = null;
        await logout().catch(() => {});
        setStatus(null);
        setLoggedIn(false);
        void notify("已被强制下线", "管理员已将你强制下线,请重新登录后再连接。");
        return;
      }

      // 状态跃迁 → 原生通知(仅在真正切换时,避免每次轮询刷屏)。
      if (prev && prev !== s.state) {
        if (s.state === "reconnecting") {
          void notify("连接中断", "网络异常,正在自动重连…");
        } else if (s.state === "connected" && prev === "reconnecting") {
          void notify("已重新连接", "VPN 连接已恢复。");
        } else if (s.state === "error") {
          void notify("连接出错", s.last_error ?? "连接已断开。");
        }
      }
      prevStateRef.current = s.state;

      setStatus(s);
      setLoggedIn(true);
    } catch {
      // Backend unreachable: stay on whatever we have, don't crash.
    }
  }, []);

  useEffect(() => {
    refresh();
    pollRef.current = window.setInterval(refresh, POLL_MS);
    const t = window.setInterval(() => setTick((x) => x + 1), 1000);
    return () => {
      if (pollRef.current) window.clearInterval(pollRef.current);
      window.clearInterval(t);
    };
  }, [refresh]);

  // touch tick so the duration re-renders every second
  void tick;

  const onConnect = async () => {
    setBusy(true);
    try {
      await connect();
    } catch (e) {
      void notify("连接失败", String(e));
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const onDisconnect = async () => {
    setBusy(true);
    try {
      await disconnect();
    } catch (e) {
      void notify("断开失败", String(e));
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const onLogout = async () => {
    setBusy(true);
    try {
      await logout();
      setShowSettings(false);
      setLoggedIn(false);
      setStatus(null);
    } catch (e) {
      void notify("登出失败", String(e));
    } finally {
      setBusy(false);
    }
  };

  if (loggedIn === null) {
    return (
      <div className="panel boot" data-state="disconnected" data-tauri-drag-region>
        <Grain />
        <span className="boot-dot" />
        <span className="boot-text">INITIALIZING</span>
      </div>
    );
  }

  if (!loggedIn) {
    return (
      <LoginView
        onLoggedIn={() => {
          setLoggedIn(true);
          refresh();
        }}
      />
    );
  }

  const state: ConnState = status?.state ?? "disconnected";
  const meta = STATE_META[state];
  const connected = state === "connected";
  const reconnecting = state === "reconnecting";
  // 重连时隧道仍在(boringtun 自动重握手),允许手动断开;仅首次建连禁用按钮。
  const tunnelUp = connected || reconnecting;
  const connecting = state === "connecting";

  return (
    <div className="panel" data-state={state} data-tauri-drag-region>
      <Grain />

      <header className="topbar" data-tauri-drag-region>
        <div className="mark" data-tauri-drag-region>
          <span className="mark-node" />
          <span className="mark-name">CIPHER</span>
        </div>
        <button
          className="gear"
          title="设置"
          aria-label="设置"
          onClick={() => setShowSettings((v) => !v)}
        >
          <svg viewBox="0 0 24 24" width="15" height="15" aria-hidden="true">
            <path
              fill="currentColor"
              d="M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Zm0 5.5a2 2 0 1 1 0-4 2 2 0 0 1 0 4Z"
            />
            <path
              fill="currentColor"
              d="m20.3 13.6-.1-1.6.1-1.6 1.6-1.3-1.6-2.8-2 .6-1.4-.8-.5-2H9.6l-.5 2-1.4.8-2-.6L4.1 9l1.6 1.3-.1 1.6.1 1.6L4.1 16l1.6 2.8 2-.6 1.4.8.5 2h2.8l.5-2 1.4-.8 2 .6 1.6-2.8-1.6-1.4Z"
              opacity=".0"
            />
          </svg>
        </button>
      </header>

      <section className="stage">
        <SignalOrb state={state} />
        <div className="state-word">{meta.word}</div>
        <div className="state-sub">
          {state === "error" && status?.last_error
            ? status.last_error
            : meta.sub}
        </div>
      </section>

      <div className="readouts">
        <Readout label="ADDR" value={status?.vpn_ip ?? "—"} mono />
        <Readout
          label="UPTIME"
          value={status?.since ? formatDuration(status.since) : "—"}
          mono
        />
        <Readout label="↓ RECV" value={formatBytes(status?.bytes_rx ?? 0)} mono />
        <Readout label="↑ SENT" value={formatBytes(status?.bytes_tx ?? 0)} mono />
      </div>

      {reconnecting && (
        <div className="hint">网络异常,正在自动重连,隧道保持中…</div>
      )}

      <button
        className={`exec ${tunnelUp ? "is-down" : "is-up"}`}
        disabled={busy || connecting}
        onClick={tunnelUp ? onDisconnect : onConnect}
      >
        {busy || connecting ? (
          <span className="spinner" />
        ) : (
          <>
            <span className="exec-bracket">[</span>
            {tunnelUp ? "断开连接" : "建立连接"}
            <span className="exec-bracket">]</span>
          </>
        )}
      </button>

      {showSettings && (
        <>
          <div className="sheet-scrim" onClick={() => setShowSettings(false)} />
          <div className="sheet" role="dialog" aria-label="设置">
            <div className="sheet-title">CONFIG</div>
            <ServerSetting />
            <button
              className="ghost"
              onClick={() => {
                setShowSettings(false);
                setShowChangePwd(true);
              }}
            >
              修改密码
            </button>
            <div className="sheet-actions">
              <button className="ghost" disabled={busy} onClick={onLogout}>
                登出
              </button>
              <button className="ghost danger" onClick={() => quitApp()}>
                退出 App
              </button>
            </div>
          </div>
        </>
      )}

      {showChangePwd && (
        <ChangePasswordSheet
          onClose={() => setShowChangePwd(false)}
          onChanged={async () => {
            setShowChangePwd(false);
            await logout().catch(() => {});
            setStatus(null);
            setLoggedIn(false);
          }}
        />
      )}
    </div>
  );
}

function SignalOrb({ state }: { state: ConnState }) {
  return (
    <div className="orb" data-state={state} aria-hidden="true">
      <span className="orb-ring r1" />
      <span className="orb-ring r2" />
      <span className="orb-ring r3" />
      <span className="orb-sweep" />
      <span className="orb-core" />
    </div>
  );
}

function Readout({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="cell">
      <span className="cell-label">{label}</span>
      <span className={`cell-value${mono ? " mono" : ""}`}>{value}</span>
    </div>
  );
}

function ServerSetting() {
  const [server, setServer] = useState<string>("");
  useEffect(() => {
    savedServer().then((s) => setServer(s ?? ""));
  }, []);
  return (
    <div className="cell wide">
      <span className="cell-label">ENDPOINT</span>
      <span className="cell-value mono">{server || "—"}</span>
    </div>
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

  // 与服务端口径一致:≥8 位 + 含字母 + 含数字。
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
      <div className="sheet-scrim" onClick={() => !busy && onClose()} />
      <div className="sheet" role="dialog" aria-label="修改密码">
        <div className="sheet-title">修改密码</div>
        <form className="auth-form" onSubmit={submit} style={{ gap: 11 }}>
          <Field label="当前密码">
            <input
              type="password"
              value={cur}
              autoFocus
              onChange={(e) => setCur(e.target.value)}
            />
          </Field>
          <Field label="新密码 · 至少 8 位且含字母与数字">
            <input
              type="password"
              value={next}
              onChange={(e) => setNext(e.target.value)}
            />
          </Field>
          <Field label="确认新密码">
            <input
              type="password"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
            />
          </Field>
          {err && <div className="auth-err">{err}</div>}
          <button
            type="submit"
            className="exec is-up"
            disabled={busy || !valid}
            style={{ marginTop: 4 }}
          >
            {busy ? (
              <span className="spinner" />
            ) : (
              <>
                <span className="exec-bracket">[</span>
                确认修改
                <span className="exec-bracket">]</span>
              </>
            )}
          </button>
          <button
            type="button"
            className="ghost"
            disabled={busy}
            onClick={onClose}
          >
            取消
          </button>
        </form>
      </div>
    </>
  );
}

function LoginView({ onLoggedIn }: { onLoggedIn: () => void }) {
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
    <div className="panel auth" data-state="disconnected" data-tauri-drag-region>
      <Grain />
      <header className="topbar" data-tauri-drag-region>
        <div className="mark" data-tauri-drag-region>
          <span className="mark-node" />
          <span className="mark-name">CIPHER</span>
        </div>
      </header>

      <div className="auth-head">
        <div className="auth-title">ESTABLISH IDENTITY</div>
        <div className="auth-tag">authenticate to open a secure channel</div>
      </div>

      <form className="auth-form" onSubmit={submit}>
        <Field label="ENDPOINT">
          <input
            type="text"
            value={server}
            placeholder="https://vpn.example.com"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            onChange={(e) => setServer(e.target.value)}
          />
        </Field>
        <Field label="OPERATOR">
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
        <Field label="KEY">
          <input
            type="password"
            value={password}
            placeholder="••••••••"
            onChange={(e) => setPassword(e.target.value)}
          />
        </Field>

        {err && <div className="auth-err">{err}</div>}

        <button
          type="submit"
          className="exec is-up"
          disabled={busy || !server || !username || !password}
        >
          {busy ? (
            <span className="spinner" />
          ) : (
            <>
              <span className="exec-bracket">[</span>
              登录
              <span className="exec-bracket">]</span>
            </>
          )}
        </button>
      </form>
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
      <div className="field-input">{children}</div>
    </label>
  );
}

function Grain() {
  return <div className="grain" aria-hidden="true" />;
}
