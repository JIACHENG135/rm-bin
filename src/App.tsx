import { useCallback, useEffect, useRef, useState } from "react";

import Widget, { DOODLES, type Phase } from "./Widget";
import Stack, { DEAD_PX, TEAR_PX, type Drag } from "./Stack";
import {
  CAP,
  IS_TAURI,
  clearPending,
  deviceOnline,
  enqueueFiles,
  enqueuePaths,
  flushQueue,
  geometryFor,
  loadPending,
  onMenu,
  onProgress,
  onQueue,
  onResult,
  openSettings,
  removePending,
  setInteractive,
  restorePending,
  showContextMenu,
  startDragging,
  warmUp,
  type PendingItem,
  type Placed,
} from "./pending";

/* Kept in step with the Rust whitelist. Used only to decide, on `dragenter`,
   whether to shake — the authoritative refusal happens on the way in. */
const IMAGE_EXT = new Set([
  "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff",
]);
const isImagePath = (p: string) =>
  IMAGE_EXT.has(p.split(".").pop()?.toLowerCase() ?? "");

/** Under this a press is a click; over it, it starts charging. */
const HOLD_MS = 150;
/** How long the charge has to be held. Long enough to be a decision. */
const CHARGE_MS = 900;
/** Offline, the ring stops here: firm, and obviously not stuck. */
const OFFLINE_CAP = 0.35;
/** So the release that fires a send does not also toggle the window. */
const SUPPRESS_CLICK_MS = 80;
/** Matches the card exit in the stylesheet. */
const TEAR_EXIT_MS = 1150;
/** How long a torn card can be put back. */
const UNDO_MS = 4000;
/** The e-ink panel settling before the check is written. */
const FLASH_MS = 500;
/** How long the receipt stays out before the window folds itself away. */
const RECEIPT_MS = 6000;
/** The pile lifting off, before the tablet's own darkness starts. */
const DRAIN_MS = 700;
const SHAKE_MS = 420;
/** Cards flying home, before the pointer area is allowed to close in. */
const COLLAPSE_MS = 480;
/** The window's fixed size — see `tauri.conf.json` and `chrome.rs`. Wide and
    tall enough for the largest layout plus room to pull a card clear of it. */
const WINDOW = { w: 320, h: 560 };

const SETTLE_SECS = 72 * 60 * 60;

function ageLabel(secs: number) {
  if (secs < 60) return "刚刚";
  if (secs < 3600) return `${Math.floor(secs / 60)} 分钟前`;
  if (secs < 86400) return `${Math.floor(secs / 3600)} 小时前`;
  return `${Math.floor(secs / 86400)} 天前`;
}

export default function App() {
  const [items, setItems] = useState<PendingItem[]>([]);
  const [phase, setPhase] = useState<Phase>("idle");
  const [charge, setCharge] = useState(0);
  const [online, setOnline] = useState(true);
  const [expanded, setExpanded] = useState(false);

  const [hover, setHover] = useState(false);
  const [hoverId, setHoverId] = useState<string | null>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [tearing, setTearing] = useState<string[]>([]);
  const [removed, setRemoved] = useState<{
    item: PendingItem;
    index: number;
  } | null>(null);

  const [receipt, setReceipt] = useState<Placed | null>(null);
  const [receiptList, setReceiptList] = useState<Placed[] | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [doodle, setDoodle] = useState<number | null>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  const raf = useRef(0);
  const holdTimer = useRef(0);
  const undoTimer = useRef(0);
  const shrinkTimer = useRef(0);
  const tearTimers = useRef(new Map<string, number>());
  const dragRef = useRef<Drag | null>(null);
  const dragFrom = useRef({ x: 0, y: 0 });
  const pressFrom = useRef({ x: 0, y: 0 });
  const pressing = useRef(false);
  const charging = useRef(false);
  const chargeRef = useRef(0);
  /** the release that sent must not also be read as a click */
  const suppressClick = useRef(false);
  const live = useRef({ items, phase, online, expanded });
  live.current = { items, phase, online, expanded };

  const n = items.length;
  const busy =
    phase === "render" ||
    phase === "drain" ||
    phase === "restart" ||
    phase === "flash";
  /* Charging closes the window: whatever is spread out is about to be sent,
     and holding it open would be showing you a thing that no longer exists.
     The undo corner keeps it open a moment longer than the queue does, so a
     tear that empties the queue is still reversible. */
  const open =
    (expanded && n > 0 && !busy && phase !== "charging") ||
    (removed !== null && !busy);
  const geom = geometryFor(n, open);
  /* The cards are laid out for the open window whatever the window is doing.
     Their positions must not depend on `open`, or they shift under the
     animation that is meant to be carrying them. */
  const layout = geometryFor(n, true);

  const oldest = n ? Math.min(...items.map((i) => i.added_at)) : 0;
  const age = n ? now - oldest : 0;
  const settled = n > 0 && age > SETTLE_SECS;

  /* ————— backend wiring ————— */

  useEffect(() => {
    let dead = false;
    const off: Array<() => void> = [];
    void (async () => {
      await warmUp();
      const state = await loadPending();
      if (dead) return;
      setItems(state.items);
      if (state.skipped > 0) {
        setNote(`已跳过 ${state.skipped} 项（源文件不在了）`);
      }
      off.push(
        await onQueue((s) => setItems(s.items)),
        await onProgress((p) => {
          if (p.stage === "render") {
            // A is the whole of the local work, so it owns the whole ring
            setCharge(Math.min(1, p.frac / 0.6));
          } else if (p.stage === "upload") {
            // the paper comes off the desk *before* the screen goes dark:
            // first the thing leaves your hands, then the device goes busy
            setCharge(1);
            setPhase("drain");
            window.setTimeout(() => setPhase("restart"), DRAIN_MS);
          }
        }),
        await onResult((r) => {
          if (!r.ok) {
            console.error(r.error);
            setPhase("invalid");
            setCharge(0);
            setNote(r.error);
            window.setTimeout(
              () => setPhase(live.current.items.length ? "pending" : "idle"),
              SHAKE_MS
            );
            return;
          }
          setPhase("flash");
          window.setTimeout(() => {
            setPhase("done");
            setCharge(0);
            setReceipt(r.placed[r.placed.length - 1] ?? null);
            setReceiptList(r.placed);
            window.setTimeout(() => {
              setPhase("idle");
              setReceipt(null);
              setReceiptList(null);
            }, RECEIPT_MS);
          }, FLASH_MS);
        }),
        await onMenu((action) => {
          if (action === "queue-clear") void clearPending();
        })
      );
      if (dead) off.forEach((f) => f());
    })();
    return () => {
      dead = true;
      off.forEach((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /* ————— how much of the window is real —————
     The window never changes size, so nothing here can make it flicker. What
     changes is only how much of it catches the pointer: the device alone when
     shut, the whole open layout when the paper is out, and everything while a
     card is being dragged, so the gesture can travel past the edge of the
     paper without being handed to the desktop mid-drag.

     Widening is immediate; narrowing waits for the cards to land, or letting
     go just outside the pile would drop the pointer through the floor. */
  useEffect(() => {
    clearTimeout(shrinkTimer.current);
    const base =
      open && receiptList
        ? { w: 112, h: 148 + 14 + Math.min(receiptList.length, 8) * 14 }
        : geometryFor(n, open);
    if (drag) {
      void setInteractive(WINDOW.w, WINDOW.h);
      return;
    }
    if (open) {
      void setInteractive(base.w, base.h);
      return;
    }
    shrinkTimer.current = window.setTimeout(
      () => void setInteractive(base.w, base.h),
      COLLAPSE_MS
    );
    return () => clearTimeout(shrinkTimer.current);
  }, [open, n, receiptList, drag !== null]);

  /* ————— is the tablet there ————— */
  useEffect(() => {
    if (busy) return;
    let dead = false;
    const poll = async () => {
      const ok = await deviceOnline();
      if (!dead) setOnline(ok);
    };
    void poll();
    const every = open ? 5000 : n > 0 ? 10000 : 30000;
    const iv = window.setInterval(poll, every);
    return () => {
      dead = true;
      clearInterval(iv);
    };
  }, [open, n, busy]);

  useEffect(() => {
    const iv = window.setInterval(
      () => setNow(Math.floor(Date.now() / 1000)),
      30000
    );
    return () => clearInterval(iv);
  }, []);

  /* ————— send ————— */

  const send = useCallback(async () => {
    setPhase("render");
    setCharge(0);
    setReceipt(null);
    setReceiptList(null);
    setExpanded(false);
    setHoverId(null);
    setNote(null);
    try {
      await flushQueue();
    } catch (e) {
      console.error(e);
      setPhase(live.current.items.length ? "pending" : "idle");
    }
  }, []);

  const beginCharge = useCallback(() => {
    const { items: cur, online: up } = live.current;
    /* whatever is spread out is on its way to the tablet — put it away */
    setExpanded(false);
    setHoverId(null);
    charging.current = true;
    chargeRef.current = 0;
    setPhase("charging");
    setCharge(0);
    /* offline the ring simply cannot fill — the refusal is in the hand, not
       in a dialog, and it costs nothing to find out */
    const cap = up ? 1 : OFFLINE_CAP;
    const t0 = performance.now();
    const step = () => {
      if (!charging.current) return;
      chargeRef.current = Math.min(cap, (performance.now() - t0) / CHARGE_MS);
      setCharge(chargeRef.current);
      raf.current = requestAnimationFrame(step);
    };
    raf.current = requestAnimationFrame(step);
    void cur;
  }, []);

  const startPress = useCallback(
    (e: React.PointerEvent) => {
      pressFrom.current = { x: e.screenX, y: e.screenY };
      pressing.current = true;
      const { items: cur, phase: p } = live.current;
      if (!cur.length || (p !== "idle" && p !== "pending" && p !== "armed")) {
        return;
      }
      /* A press is not yet a gesture. Under 150ms it turns out to have been a
         click; past that it commits to charging, which is why the ring never
         flickers on during an ordinary click. */
      holdTimer.current = window.setTimeout(beginCharge, HOLD_MS);
    },
    [beginCharge]
  );

  /** Let go early and nothing happened — which is why there is no second
      confirmation anywhere in this flow. Cancelling costs zero. */
  const cancelCharge = useCallback(() => {
    clearTimeout(holdTimer.current);
    if (!charging.current) return;
    charging.current = false;
    cancelAnimationFrame(raf.current);
    chargeRef.current = 0;
    setCharge(0);
    setPhase(live.current.items.length ? "pending" : "idle");
  }, []);

  const endPress = useCallback(() => {
    clearTimeout(holdTimer.current);
    pressing.current = false;
    if (!charging.current) return;
    charging.current = false;
    cancelAnimationFrame(raf.current);
    const held = chargeRef.current;
    chargeRef.current = 0;
    setCharge(0);
    // a release that did something must not also toggle the window open
    suppressClick.current = true;
    window.setTimeout(() => {
      suppressClick.current = false;
    }, SUPPRESS_CLICK_MS);

    if (held >= 1) {
      void send();
    } else if (!live.current.online) {
      // it stopped at 35% and would not go further; the shake says so again
      setPhase("invalid");
      window.setTimeout(
        () => setPhase(live.current.items.length ? "pending" : "idle"),
        SHAKE_MS
      );
    } else {
      setPhase(live.current.items.length ? "pending" : "idle");
    }
  }, [send]);

  /* The device is also the window's handle. A press that stays put is a click
     or a send; a press that travels is someone moving the bin on their
     desktop. The window drag cannot be declarative (`data-tauri-drag-region`)
     because that claims the mousedown before any of this gets to run. */
  useEffect(() => {
    const move = (e: PointerEvent) => {
      if (!pressing.current) return;
      const { x, y } = pressFrom.current;
      if (Math.hypot(e.screenX - x, e.screenY - y) <= DEAD_PX) return;
      pressing.current = false;
      suppressClick.current = true;
      window.setTimeout(() => {
        suppressClick.current = false;
      }, SUPPRESS_CLICK_MS);
      cancelCharge();
      void startDragging();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", endPress);
    window.addEventListener("pointercancel", endPress);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", endPress);
      window.removeEventListener("pointercancel", endPress);
    };
  }, [cancelCharge, endPress]);

  const toggle = useCallback(() => {
    if (suppressClick.current) return;
    const { items: cur, phase: p } = live.current;
    if (!cur.length || p === "charging") return;
    if (p === "render" || p === "drain" || p === "restart" || p === "flash") {
      return;
    }
    setExpanded((v) => !v);
    setHoverId(null);
  }, []);

  /* The keyboard has no "hold", so it does not pretend to: ⌘↩ is the plain
     equivalent, and cancelling it is simply not pressing it. */
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.key !== "Enter") return;
      const { items: cur, phase: p, online: up } = live.current;
      if (!cur.length || !up || (p !== "idle" && p !== "pending")) return;
      e.preventDefault();
      void send();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [send]);

  /* ————— tearing a card out ————— */

  const tear = useCallback(
    (id: string) => {
      const index = items.findIndex((i) => i.id === id);
      const item = items[index];
      if (!item) return;
      setTearing((t) => [...t, id]);
      const timer = window.setTimeout(() => {
        tearTimers.current.delete(id);
        void removePending(id);
        setTearing((t) => t.filter((x) => x !== id));
      }, TEAR_EXIT_MS);
      tearTimers.current.set(id, timer);
      setRemoved({ item, index });
      clearTimeout(undoTimer.current);
      undoTimer.current = window.setTimeout(() => setRemoved(null), UNDO_MS);
    },
    [items]
  );

  const undo = useCallback(() => {
    if (!removed) return;
    const { item, index } = removed;
    clearTimeout(undoTimer.current);
    setRemoved(null);
    const timer = tearTimers.current.get(item.id);
    if (timer !== undefined) {
      // still mid-exit: nothing has left yet, so just stop it leaving
      clearTimeout(timer);
      tearTimers.current.delete(item.id);
      setTearing((t) => t.filter((x) => x !== item.id));
    } else {
      void restorePending(item, index);
    }
  }, [removed]);

  useEffect(() => {
    const move = (e: PointerEvent) => {
      const d = dragRef.current;
      if (!d) return;
      const dx = e.screenX - dragFrom.current.x;
      const dy = e.screenY - dragFrom.current.y;
      // below the dead zone this is still a hover with a twitch in it
      const next =
        Math.hypot(dx, dy) < DEAD_PX ? { ...d, dx: 0, dy: 0 } : { ...d, dx, dy };
      dragRef.current = next;
      setDrag(next);
    };
    const up = () => {
      const d = dragRef.current;
      if (!d) return;
      dragRef.current = null;
      setDrag(null);
      if (Math.hypot(d.dx, d.dy) > TEAR_PX) tear(d.id);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  }, [tear]);

  /* ————— what comes in ————— */

  const flashInvalid = useCallback(() => {
    setPhase("invalid");
    window.setTimeout(
      () => setPhase(live.current.items.length ? "pending" : "idle"),
      SHAKE_MS
    );
  }, []);

  const accept = useCallback(
    async (paths: string[]) => {
      const state = await enqueuePaths(paths);
      setItems(state.items);
      if (state.rejected > 0) {
        // one shake for the whole drop, however many were turned away, and no
        // filenames: dropping the wrong file is something you already know
        flashInvalid();
        setNote(`${state.rejected} 张读不出来`);
      } else {
        setPhase(state.items.length ? "pending" : "idle");
      }
    },
    [flashInvalid]
  );

  useEffect(() => {
    if (!IS_TAURI) return;
    let unlisten: (() => void) | undefined;
    let dead = false;
    void (async () => {
      const { getCurrentWebviewWindow } = await import(
        "@tauri-apps/api/webviewWindow"
      );
      const un = await getCurrentWebviewWindow().onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "enter") {
          // the refusal lands before you let go, so walking away is free
          setPhase(p.paths.some(isImagePath) ? "armed" : "invalid");
        } else if (p.type === "drop") {
          void accept(p.paths);
        } else if (p.type === "leave") {
          setPhase(live.current.items.length ? "pending" : "idle");
        }
      });
      if (dead) un();
      else unlisten = un;
    })();
    return () => {
      dead = true;
      unlisten?.();
    };
  }, [accept]);

  /* browser fallback, so the timing can be worked on without a tablet */
  useEffect(() => {
    if (IS_TAURI) return;
    const over = (e: DragEvent) => {
      e.preventDefault();
      setPhase("armed");
    };
    const leave = () => setPhase(live.current.items.length ? "pending" : "idle");
    const drop = async (e: DragEvent) => {
      e.preventDefault();
      const files = Array.from(e.dataTransfer?.files ?? []);
      if (!files.length) return;
      const state = await enqueueFiles(files);
      setItems(state.items);
      if (state.rejected > 0) {
        flashInvalid();
        setNote(`${state.rejected} 张读不出来`);
      } else {
        setPhase(state.items.length ? "pending" : "idle");
      }
    };
    window.addEventListener("dragover", over);
    window.addEventListener("dragleave", leave);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragover", over);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("drop", drop);
    };
  }, [flashInvalid]);

  /* A delivery is the one time the window opens by itself — the receipt put
     down on the desk. It closes again on its own, or the moment you leave. */
  useEffect(() => {
    if (receiptList) setExpanded(true);
  }, [receiptList]);
  useEffect(() => {
    if (receiptList && !hover) setExpanded(false);
  }, [receiptList, hover]);

  /* the note has been read by the time the window shuts */
  useEffect(() => {
    if (!open && note) {
      const t = window.setTimeout(() => setNote(null), 400);
      return () => clearTimeout(t);
    }
  }, [open, note]);

  /* while truly idle, the Marker doodles in the corner now and then */
  const idle = n === 0 && phase === "idle" && !receipt;
  useEffect(() => {
    if (!idle) {
      setDoodle(null);
      return;
    }
    let i = 0;
    let hide = 0;
    const iv = window.setInterval(() => {
      setDoodle(i % DOODLES.length);
      i++;
      hide = window.setTimeout(() => setDoodle(null), 3600);
    }, 8000);
    return () => {
      clearInterval(iv);
      clearTimeout(hide);
    };
  }, [idle]);

  useEffect(
    () => () => {
      cancelAnimationFrame(raf.current);
      clearTimeout(holdTimer.current);
      clearTimeout(undoTimer.current);
      clearTimeout(shrinkTimer.current);
      tearTimers.current.forEach(clearTimeout);
    },
    []
  );

  const showReceipt = open && receiptList !== null;

  /* The open window has exactly one free strip — 12px above the top card —
     so the things it will say have to queue up for it, most actionable
     first. None of them is worth pushing a card out of the way for. */
  const status = note
    ? note
    : !online
    ? "设备离线 · 检查连接"
    : n > CAP
    ? `+${n - CAP} 张未显示 · 最早 ${ageLabel(age)}`
    : n
    ? `最早 ${ageLabel(age)}`
    : "";

  return (
    <div
      className="wrap"
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => {
        setHover(false);
        setHoverId(null);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        void showContextMenu();
      }}
    >
      {!showReceipt && (
        <Stack
          items={items}
          geom={layout}
          tearing={tearing}
          open={open}
          hoverId={hoverId}
          drag={drag}
          onCardDown={(id, e) => {
            e.stopPropagation();
            dragFrom.current = { x: e.screenX, y: e.screenY };
            dragRef.current = { id, dx: 0, dy: 0 };
            setDrag(dragRef.current);
          }}
          onCardEnter={setHoverId}
          onCardLeave={() => setHoverId(null)}
        />
      )}

      {open && !showReceipt && status && (
        <div
          className="status-row"
          style={{ bottom: `${geom.h - 11}px`, width: `${geom.w - 4}px` }}
          data-actionable={!online}
          onClick={
            online
              ? undefined
              : (e) => {
                  e.stopPropagation();
                  void openSettings();
                  void deviceOnline().then(setOnline);
                }
          }
        >
          {status}
        </div>
      )}

      {/* the delivered index, laid out like a receipt */}
      {showReceipt && (
        <div className="receipt-list">
          {receiptList.slice(0, 8).map((r, i) => (
            <div
              className="receipt-row"
              key={`${r.folder}/${r.name}/${i}`}
              style={{ animationDelay: `${i * 70}ms` }}
            >
              <span className="folder">{r.folder}</span>
              <span className="doc">{r.name}</span>
            </div>
          ))}
          {receiptList.length > 8 && (
            <div className="receipt-more">…及其余 {receiptList.length - 8} 份</div>
          )}
        </div>
      )}

      {/* a torn-off corner left on the table for four seconds */}
      {removed !== null && (
        <div
          className="undo-corner"
          onClick={(e) => {
            e.stopPropagation();
            undo();
          }}
        />
      )}

      <div className="stage" onPointerDown={startPress} onClick={toggle}>
        <Widget
          items={items}
          phase={phase}
          charge={charge}
          online={online}
          hover={hover || open}
          receipt={receipt}
          settled={settled}
          doodle={doodle}
          hideStack={open}
        />
      </div>
    </div>
  );
}
