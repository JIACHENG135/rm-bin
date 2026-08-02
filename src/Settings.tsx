import { useCallback, useEffect, useRef, useState } from "react";

const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Kept in step with `settings::Mode` on the Rust side. */
type Mode = "pen" | "file" | "screen" | "sketch" | "pdf";

const DEFAULTS = { host: "10.11.99.1", port: 22, mode: "pen" as Mode };

type Config = { host: string; port: number; mode: Mode };

/** The two ways a drawing can get onto the tablet, and what each costs. */
const MODES: { id: Mode; title: string; hint: string; note: string }[] = [
  {
    id: "pen",
    title: "笔重放",
    hint: "边画边看",
    note: "假装成设备的笔，一笔一笔写进去——窗口里的小屏幕跟着真机同步画出同一幅。画在当前打开的那一页上，内容多时要几分钟。",
  },
  {
    id: "file",
    title: "写入笔记本",
    hint: "秒级完成",
    note: "直接生成一本新笔记送过去，几秒完成，落在干净的新页上。设备会重启一次界面才能看到它，窗口里的绘制是事后回放。",
  },
  {
    id: "screen",
    title: "直接显示",
    hint: "原图灰阶",
    note: "不描线，把原图按 1620×2160 直接画到屏幕上——照片就是照片，有完整的灰阶层次。代价是它不是文档：显示期间设备界面被暂停，内容不会保存，十分钟后自动恢复（也可在右键菜单里立刻恢复）。",
  },
  {
    id: "sketch",
    title: "Gemini 线稿 + 笔重放",
    hint: "照片可用",
    note: "先让 Gemini 把图重画成干净线稿，再用笔一笔笔写上去。照片终于能出好结果——但落到纸上的是一幅「照着画的画」，不是原图；本来就干净的线稿别用这个。需要环境变量 GEMINI_API_KEY（从访达启动的话，要先 launchctl setenv）。",
  },
  {
    id: "pdf",
    title: "存成 PDF",
    hint: "原图 · 可批注",
    note: "把图片包成单页 PDF 交给设备自己的导入接口。唯一同时做到「是原图、是文档、能用笔批注」的一种，而且不用 SSH、不用停设备界面。前提是设备上打开了「设置 › 通用 › 存储 › USB 网页界面」，并用 USB 线连接（地址 10.11.99.1）。",
  },
];
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

  /* Mode is a click, not a field: it commits the moment it changes, and it
     carries the last *saved* host rather than whatever is being typed. */
  const chooseMode = async (mode: Mode) => {
    if (mode === config.mode) return;
    const next = { ...config, mode };
    setConfig(next);
    try {
      if (IS_TAURI) setConfig(await call<Config>("save_settings", { settings: next }));
    } catch (e) {
      setError(String(e));
    }
  };

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

        <h2 className="group-title">绘制方式</h2>

        <div className="group">
          {MODES.map((m, i) => (
            <div key={m.id}>
              {i > 0 && <div className="separator" />}
              <label className="choice">
                <input
                  type="radio"
                  name="mode"
                  checked={config.mode === m.id}
                  onChange={() => chooseMode(m.id)}
                />
                <span className="choice-title">{m.title}</span>
                <span className="choice-hint">{m.hint}</span>
              </label>
            </div>
          ))}
        </div>

        <p className="footnote">
          {MODES.find((m) => m.id === config.mode)?.note}
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
