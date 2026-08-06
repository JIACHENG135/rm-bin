/**
 * The window's side of the pending queue.
 *
 * Everything the UI needs from the backend goes through here, including a
 * complete stand-in for it. The stand-in is not a test fixture — it is how the
 * app is worked on: `npm run dev` in a browser has no tablet to interrupt and
 * no ssh to wait for, and the whole redesign is about timing that you can only
 * judge by watching it. So the mock runs the same phase sequence on the same
 * clock, and `App.tsx` cannot tell the difference.
 */

export const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export type PendingItem = {
  id: string;
  path: string;
  name: string;
  /** absolute path on disk; run it through `thumbUrl` before using as a src */
  thumb: string;
  tint: string;
  mtime: number;
  size: number;
  /** unix seconds */
  added_at: number;
};

export type QueueState = {
  items: PendingItem[];
  /** dropped on load because neither original nor thumbnail survived */
  skipped: number;
  /** turned away in the last drop */
  rejected: number;
};

export type Placed = { name: string; folder: string };

/** `render` is this machine working; `upload` is the tablet's own time. */
export type Progress = { stage: "render" | "upload" | "done"; frac: number };

export type SendResult = {
  ok: boolean;
  route: "web" | "ssh" | null;
  placed: Placed[];
  skipped: number;
  error: string | null;
};

const EMPTY: QueueState = { items: [], skipped: 0, rejected: 0 };

/* ————— window geometry —————
   One window, two sizes. It grows *upward* out of a device that does not move,
   so the paper reads as coming out of the tablet rather than as a panel
   appearing next to it.

   Collapsed  112 x 148
   Fan (<=6)  112 x (206 + (rows-1) * 46)
   Grid (>6)  240 x (206 + (rows-1) * 96), three columns

   The window itself is fixed at a size that fits the largest of these and is
   never resized; these numbers only drive where the cards sit and how much of
   the window catches the pointer. */

/** Card footprint, and the gaps the fan and the grid step by. */
export const CARD = { w: 68, h: 88 };
export const FAN_STEP = 46;
export const GRID_STEP = 96;
export const COL_STEP = 76;
/** Past twelve the window would be taller than most of the screen. */
export const CAP = 12;
export const CELL = { w: 112, h: 148 };
/** Where the lowest card sits, measured from the window's bottom edge. */
export const FLOOR = 106;

export type Geometry = {
  w: number;
  h: number;
  /** false once there are more than six — the fan becomes a grid */
  fan: boolean;
  cols: number;
  rows: number;
  /** how many cards are actually drawn */
  shown: number;
  /** left edge of the first column */
  gx0: number;
};

export function geometryFor(count: number, expanded: boolean): Geometry {
  const shown = Math.min(count, CAP);
  const fan = shown <= 6;
  const cols = fan ? 1 : 3;
  const rows = Math.max(1, Math.ceil(shown / cols));
  const gridW = cols * CARD.w + (cols - 1) * (COL_STEP - CARD.w);
  const w = expanded ? (fan ? CELL.w : 240) : CELL.w;
  const h = expanded ? 206 + (rows - 1) * (fan ? FAN_STEP : GRID_STEP) : CELL.h;
  return { w, h, fan, cols, rows, shown, gx0: (w - gridW) / 2 };
}

let declared: { w: number; h: number } | null = null;

/**
 * Tell the backend how much of the window is real.
 *
 * Not a resize — the window is one fixed size for its whole life, because
 * resizing a transparent always-on-top window is what made it blink. This is
 * only about which part of it should catch the pointer; everything outside
 * the box lets clicks through to the desktop.
 */
export async function setInteractive(w: number, h: number) {
  if (!IS_TAURI) return;
  if (declared && declared.w === w && declared.h === h) return;
  declared = { w, h };
  try {
    await (await core()).invoke("set_interactive_rect", { w, h });
  } catch (e) {
    console.error("set_interactive_rect failed", e);
    declared = null;
  }
}

/* ————— the real backend ————— */

async function core() {
  return import("@tauri-apps/api/core");
}

export function thumbUrl(path: string) {
  // the mock hands back blob: URLs, which are already loadable
  if (!IS_TAURI || path.startsWith("blob:")) return path;
  return convertCache(path);
}

let convertFileSrc: ((p: string) => string) | null = null;
const converted = new Map<string, string>();
function convertCache(path: string) {
  const hit = converted.get(path);
  if (hit) return hit;
  if (!convertFileSrc) {
    // first call races the dynamic import; the retry on the next render wins
    void core().then((m) => {
      convertFileSrc = m.convertFileSrc;
    });
    return "";
  }
  const url = convertFileSrc(path);
  converted.set(path, url);
  return url;
}

/** Prime the asset-protocol converter so the first thumbnails are not blank. */
export async function warmUp() {
  if (!IS_TAURI) return;
  convertFileSrc = (await core()).convertFileSrc;
}

/* ————— commands ————— */

export async function loadPending(): Promise<QueueState> {
  if (!IS_TAURI) return mock.load();
  return (await core()).invoke<QueueState>("load_pending");
}

export async function enqueuePaths(paths: string[]): Promise<QueueState> {
  if (!IS_TAURI) return EMPTY;
  return (await core()).invoke<QueueState>("enqueue_images", { paths });
}

/** Browser-only: the dev drop hands over `File`s, never paths. */
export async function enqueueFiles(files: File[]): Promise<QueueState> {
  return mock.add(files);
}

export async function removePending(id: string): Promise<QueueState> {
  if (!IS_TAURI) return mock.remove(id);
  return (await core()).invoke<QueueState>("remove_pending", { id });
}

export async function restorePending(
  item: PendingItem,
  index: number
): Promise<QueueState> {
  if (!IS_TAURI) return mock.restore(item, index);
  return (await core()).invoke<QueueState>("restore_pending", { item, index });
}

export async function clearPending(): Promise<QueueState> {
  if (!IS_TAURI) return mock.clear();
  return (await core()).invoke<QueueState>("clear_pending");
}

export async function flushQueue(): Promise<void> {
  if (!IS_TAURI) return mock.send();
  await (await core()).invoke("flush_queue");
}

export async function deviceOnline(): Promise<boolean> {
  if (!IS_TAURI) return mock.online;
  return (await core()).invoke<boolean>("device_online");
}

/**
 * Start moving the window under the pointer.
 *
 * This is what `data-tauri-drag-region` would do declaratively — but that
 * attribute claims the mousedown before the webview sees it, and the press is
 * spoken for: holding the device is how you send. So the drag is started by
 * hand, only once a press has become a movement.
 */
export async function startDragging(): Promise<void> {
  if (!IS_TAURI) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().startDragging();
}

export async function openSettings(): Promise<void> {
  if (!IS_TAURI) return;
  await (await core()).invoke("open_settings");
}

export async function showContextMenu(): Promise<void> {
  if (!IS_TAURI) return;
  await (await core()).invoke("show_context_menu");
}

/* ————— events ————— */

type Off = () => void;

async function on<T>(name: string, cb: (payload: T) => void): Promise<Off> {
  if (!IS_TAURI) return mock.on(name, cb as (p: unknown) => void);
  const { listen } = await import("@tauri-apps/api/event");
  return listen<T>(name, (e) => cb(e.payload));
}

export const onQueue = (cb: (s: QueueState) => void) =>
  on<QueueState>("queue-changed", cb);
export const onProgress = (cb: (p: Progress) => void) =>
  on<Progress>("batch-progress", cb);
export const onResult = (cb: (r: SendResult) => void) =>
  on<SendResult>("batch-result", cb);
export const onMenu = (cb: (action: string) => void) =>
  on<string>("menu-action", cb);

/* ————— the stand-in ————— */

const NAMES: Array<[string, string]> = [
  ["算法题解", "分组背包问题"],
  ["数据结构", "B 树与 B+ 树对比"],
  ["工作笔记", "会议白板 · 排期"],
  ["算法题解", "单调栈模板"],
  ["计算机网络", "TCP 拥塞控制"],
  ["数学", "拉格朗日乘子法"],
];

const mock = new (class {
  items: PendingItem[] = [];
  online = true;
  private seq = 0;
  private listeners = new Map<string, Set<(p: unknown) => void>>();

  on(name: string, cb: (p: unknown) => void): Off {
    const set = this.listeners.get(name) ?? new Set();
    set.add(cb);
    this.listeners.set(name, set);
    return () => set.delete(cb);
  }
  private emit(name: string, payload: unknown) {
    this.listeners.get(name)?.forEach((cb) => cb(payload));
  }
  private state(rejected = 0): QueueState {
    return { items: [...this.items], skipped: 0, rejected };
  }
  private publish(rejected = 0) {
    const s = this.state(rejected);
    this.emit("queue-changed", s);
    return s;
  }

  async load() {
    return this.publish();
  }

  async add(files: File[]) {
    const good = files.filter((f) => f.type.startsWith("image/"));
    for (const f of good) {
      this.seq++;
      this.items.push({
        id: "m" + this.seq,
        path: f.name,
        name: f.name,
        thumb: URL.createObjectURL(f),
        tint: "#cfd6d8",
        mtime: 0,
        size: 0,
        added_at: Math.floor(Date.now() / 1000),
      });
    }
    return this.publish(files.length - good.length);
  }

  async remove(id: string) {
    this.items = this.items.filter((i) => i.id !== id);
    return this.publish();
  }
  async restore(item: PendingItem, index: number) {
    if (!this.items.some((i) => i.id === item.id)) {
      this.items.splice(Math.min(index, this.items.length), 0, item);
    }
    return this.publish();
  }
  async clear() {
    this.items = [];
    return this.publish();
  }

  /** Same two phases, same order, plausible durations. */
  async send() {
    const batch = [...this.items];
    const started = performance.now();
    const RENDER_MS = 2400;
    const step = () => {
      const p = Math.min(1, (performance.now() - started) / RENDER_MS);
      this.emit("batch-progress", { stage: "render", frac: p * 0.6 });
      if (p < 1) requestAnimationFrame(step);
      else {
        this.emit("batch-progress", { stage: "upload", frac: 0.6 });
        setTimeout(() => {
          this.items = [];
          this.publish();
          this.emit("batch-progress", { stage: "done", frac: 1 });
          this.emit("batch-result", {
            ok: true,
            route: "ssh",
            placed: batch.map((_, i) => {
              const [folder, name] = NAMES[i % NAMES.length];
              return { name, folder };
            }),
            skipped: 0,
            error: null,
          });
        }, 3200);
      }
    };
    requestAnimationFrame(step);
  }
})();

/** Browser-only escape hatch for trying the offline behaviour by hand. */
export function mockSetOnline(on: boolean) {
  mock.online = on;
}
