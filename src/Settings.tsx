import { useCallback, useEffect, useRef, useState } from "react";

const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const DEFAULTS = { host: "10.11.99.1", port: 22 };

type Config = { host: string; port: number };

type Probe = { ok: boolean; latency_ms: number; detail: string };
type Status =
  | { kind: "idle" }
  | { kind: "probing" }
  | { kind: "ok"; latency: number }
  | { kind: "fail"; detail: string };

/* IPv4, or a hostname such as remarkable.local */
const IPV4 =
  /^(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/;
const HOSTNAME = /^(?=.{1,253}$)([a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)(\.[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/;

function hostError(v: string): string | null {
  const s = v.trim();
  if (!s) return "请输入设备的 IP 地址。";
  if (IPV4.test(s) || HOSTNAME.test(s)) return null;
  if (/^\d+(\.\d+){3}$/.test(s)) return "IP 地址的每一段必须在 0 到 255 之间。";
  return "地址格式无效，请输入 IP 地址或主机名。";
}

const call = async <T,>(cmd: string, args?: Record<string, unknown>) => {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
};

export default function Settings() {
  const [config, setConfig] = useState<Config>(DEFAULTS);
  const [host, setHost] = useState(DEFAULTS.host);
  const [port, setPort] = useState(String(DEFAULTS.port));
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const probeSeq = useRef(0);

  /* ————— probe: TCP handshake against the device —————
     Takes only what it dials, so it can be called with a field being edited
     as easily as with the saved config. */
  const probe = useCallback(async (c: { host: string; port: number }) => {
    const seq = ++probeSeq.current;
    setStatus({ kind: "probing" });
    let r: Probe;
    if (IS_TAURI) {
      r = await call<Probe>("test_connection", { host: c.host, port: c.port });
    } else {
      await new Promise((res) => setTimeout(res, 700));
      r = { ok: false, latency_ms: 0, detail: "浏览器预览中不可用" };
    }
    if (seq !== probeSeq.current) return; // a newer probe took over
    setStatus(
      r.ok
        ? { kind: "ok", latency: r.latency_ms }
        : { kind: "fail", detail: r.detail }
    );
  }, []);

  /* ————— load ————— */
  useEffect(() => {
    (async () => {
      let c = DEFAULTS;
      if (IS_TAURI) {
        try {
          c = await call<Config>("load_settings");
        } catch (e) {
          console.error(e);
        }
      }
      setConfig(c);
      setHost(c.host);
      setPort(String(c.port));
      probe(c);
    })();
  }, [probe]);

  /* ————— instant apply, macOS-style: commit on blur / return ————— */
  const commit = useCallback(async () => {
    const nextHost = host.trim();
    const nextPort = Number(port.trim()) || DEFAULTS.port;
    const err = hostError(nextHost);
    setError(err);
    if (err) return;

    setHost(nextHost);
    setPort(String(nextPort));
    if (nextHost === config.host && nextPort === config.port) return;

    const next = { ...config, host: nextHost, port: nextPort };
    try {
      const saved = IS_TAURI
        ? await call<Config>("save_settings", { settings: next })
        : next;
      setConfig(saved);
      probe(saved);
    } catch (e) {
      setError(String(e));
    }
  }, [host, port, config, probe]);

  const revert = () => {
    setHost(config.host);
    setPort(String(config.port));
    setError(null);
  };

  const restoreDefaults = async () => {
    setHost(DEFAULTS.host);
    setPort(String(DEFAULTS.port));
    setError(null);
    try {
      const saved = IS_TAURI
        ? await call<Config>("save_settings", { settings: DEFAULTS })
        : DEFAULTS;
      setConfig(saved);
      probe(saved);
    } catch (e) {
      setError(String(e));
    }
  };

  const done = async () => {
    await commit();
    if (!IS_TAURI) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    getCurrentWindow().close();
  };

  /* Esc closes the panel, as any macOS preferences sheet does */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") done();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const fieldKeys = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.currentTarget.blur();
    } else if (e.key === "Escape") {
      e.stopPropagation();
      revert();
      e.currentTarget.blur();
    }
  };

  return (
    <div className="prefs">
      <div className="content">
        <h2 className="group-title">reMarkable 设备</h2>

        <div className="group">
          <div className="row">
            <label className="row-label" htmlFor="host">
              地址
            </label>
            <input
              id="host"
              autoFocus
              className={`field mono${error ? " invalid" : ""}`}
              value={host}
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              placeholder="10.11.99.1"
              onChange={(e) => {
                setHost(e.target.value);
                if (error) setError(null);
              }}
              onBlur={commit}
              onKeyDown={fieldKeys}
            />
          </div>

          <div className="separator" />

          <div className="row">
            <label className="row-label" htmlFor="port">
              端口
            </label>
            <input
              id="port"
              className="field mono narrow"
              value={port}
              inputMode="numeric"
              spellCheck={false}
              onChange={(e) => setPort(e.target.value.replace(/[^\d]/g, ""))}
              onBlur={commit}
              onKeyDown={fieldKeys}
            />
            <span className="row-hint">SSH，通常无需更改</span>
          </div>

          <div className="separator" />

          <div className="row">
            <span className="row-label">连接</span>
            <span className={`status ${status.kind}`}>
              {status.kind === "probing" ? (
                <>
                  <span className="spinner" aria-hidden />
                  正在检查…
                </>
              ) : status.kind === "ok" ? (
                <>
                  <span className="dot" aria-hidden />
                  已连接 · {status.latency} ms
                </>
              ) : status.kind === "fail" ? (
                <>
                  <span className="dot" aria-hidden />
                  未连接
                </>
              ) : (
                <>
                  <span className="dot" aria-hidden />
                  尚未检查
                </>
              )}
            </span>
            <button
              className="button"
              disabled={status.kind === "probing" || !!error}
              onClick={async () => {
                await commit();
                if (!hostError(host.trim()))
                  probe({ host: host.trim(), port: Number(port) || 22 });
              }}
            >
              检查连接
            </button>
          </div>
        </div>

        <p className={`footnote${error ? " error" : ""}`}>
          {error
            ? error
            : status.kind === "fail" && status.detail
            ? status.detail
            : "通过 USB 连接时地址固定为 10.11.99.1。使用 Wi-Fi 时，请在设备上打开「设置 › 通用 › 关于本机 › 版权与许可」查看 IP 地址。"}
        </p>
      </div>

      <div className="footer">
        <button className="button plain" onClick={restoreDefaults}>
          恢复默认值
        </button>
        <button className="button default" onClick={done}>
          完成
        </button>
      </div>
    </div>
  );
}
