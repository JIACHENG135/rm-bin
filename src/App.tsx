import { useEffect, useRef, useState } from "react";
import {
  AnimatePresence,
  motion,
  useMotionValue,
  useSpring,
  useTransform,
} from "framer-motion";

const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const IMAGE_EXT = new Set([
  "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "heic", "svg",
]);
const isImagePath = (p: string) =>
  IMAGE_EXT.has(p.split(".").pop()?.toLowerCase() ?? "");

type Phase = "idle" | "armed" | "invalid";
type Job = { src: string; step: "send" | "done" };

/* little idle doodles: star, wave, spiral */
const DOODLES = [
  "M12 3.5 L14.3 9.3 L20.5 9.3 L15.5 13 L17.4 19 L12 15.4 L6.6 19 L8.5 13 L3.5 9.3 L9.7 9.3 Z",
  "M3 13 C6 9, 9 9, 12 13 C15 17, 18 17, 21 13",
  "M12.5 12.5 C13.8 12.3 14.2 10.9 13 10.3 C11.2 9.5 9.4 11 9.8 12.9 C10.3 15.4 13.2 16.5 15.4 15.2 C18.2 13.6 18.7 9.8 16.4 7.6",
] as const;

const SCAN_MS = 1500; // e-ink scan duration, matches the (stub) transfer time

export default function App() {
  const [phase, setPhase] = useState<Phase>("idle");
  const [job, setJob] = useState<Job | null>(null);
  const [sent, setSent] = useState(false);
  const [hover, setHover] = useState(false);
  const [doodle, setDoodle] = useState<number | null>(null);
  const busy = useRef(false);

  /* ————— magnetic tilt: paper follows the cursor while a file hovers ————— */
  const mx = useMotionValue(0.5);
  const my = useMotionValue(0.5);
  const rotX = useSpring(useTransform(my, [0, 1], [7, -7]), {
    stiffness: 200,
    damping: 16,
  });
  const rotY = useSpring(useTransform(mx, [0, 1], [-7, 7]), {
    stiffness: 200,
    damping: 16,
  });
  const track = (x: number, y: number) => {
    mx.set(Math.min(1, Math.max(0, x / window.innerWidth)));
    my.set(Math.min(1, Math.max(0, y / window.innerHeight)));
  };
  const untrack = () => {
    mx.set(0.5);
    my.set(0.5);
  };

  const send = async (path: string, src: string) => {
    if (busy.current) return;
    busy.current = true;
    untrack();
    setJob({ src, step: "send" });
    try {
      if (IS_TAURI) {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("send_to_remarkable", { path });
      } else {
        await new Promise((r) => setTimeout(r, SCAN_MS));
      }
    } catch (e) {
      console.error(e);
    }
    setJob({ src, step: "done" });
    // a beat of stillness after the ink settles, then the page is sent off
    setTimeout(() => {
      setJob(null);
      // the resolution: the Marker writes a small check on the blank page
      setTimeout(() => {
        setSent(true);
        setTimeout(() => {
          setSent(false);
          busy.current = false;
        }, 1400);
      }, 350);
    }, 500);
  };

  /* ————— tauri native drag-drop ————— */
  useEffect(() => {
    if (!IS_TAURI) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    (async () => {
      const [{ getCurrentWebviewWindow }, { convertFileSrc }] =
        await Promise.all([
          import("@tauri-apps/api/webviewWindow"),
          import("@tauri-apps/api/core"),
        ]);
      const un = await getCurrentWebviewWindow().onDragDropEvent((event) => {
        const p = event.payload;
        const s = window.devicePixelRatio || 1;
        if (p.type === "enter") {
          setPhase(p.paths.some(isImagePath) ? "armed" : "invalid");
          track(p.position.x / s, p.position.y / s);
        } else if (p.type === "over") {
          track(p.position.x / s, p.position.y / s);
        } else if (p.type === "drop") {
          const img = p.paths.find(isImagePath);
          setPhase("idle");
          if (img) send(img, convertFileSrc(img));
          else {
            setPhase("invalid");
            setTimeout(() => setPhase("idle"), 500);
          }
          untrack();
        } else if (p.type === "leave") {
          setPhase("idle");
          untrack();
        }
      });
      if (cancelled) un();
      else unlisten = un;
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /* ————— browser fallback for vite dev ————— */
  useEffect(() => {
    if (IS_TAURI) return;
    const over = (e: DragEvent) => {
      e.preventDefault();
      setPhase("armed");
      track(e.clientX, e.clientY);
    };
    const leave = () => {
      setPhase("idle");
      untrack();
    };
    const drop = (e: DragEvent) => {
      e.preventDefault();
      setPhase("idle");
      untrack();
      const f = Array.from(e.dataTransfer?.files ?? []).find((f) =>
        f.type.startsWith("image/")
      );
      if (f) {
        const url = URL.createObjectURL(f);
        send(url, url);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const quit = async () => {
    if (!IS_TAURI) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    getCurrentWindow().close();
  };

  const armed = phase === "armed";
  const invalid = phase === "invalid";
  const idle = !job && !sent && phase === "idle";

  /* while truly idle, the Marker doodles in the corner now and then */
  useEffect(() => {
    if (!idle) {
      setDoodle(null);
      return;
    }
    let n = 0;
    let hide: number | undefined;
    const iv = window.setInterval(() => {
      setDoodle(n % 3);
      n++;
      hide = window.setTimeout(() => setDoodle(null), 3600);
    }, 8000);
    return () => {
      clearInterval(iv);
      if (hide) clearTimeout(hide);
    };
  }, [idle]);

  /* ————— sheet motion states ————— */
  const variant = invalid
    ? "invalid"
    : job
    ? "loaded"
    : sent
    ? "nod"
    : armed
    ? "armed"
    : "idle";

  return (
    <div
      className="wrap"
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <motion.div
        className="fade"
        animate={{ opacity: armed || invalid || hover || job ? 1 : 0.5 }}
        transition={{ duration: 0.45, ease: "easeOut" }}
      >
      <motion.div
        className="sheet"
        data-tauri-drag-region
        style={{ rotateX: rotX, rotateY: rotY }}
        variants={{
          /* breathing: rests small and quiet, grows when a file approaches */
          idle: {
            x: 0,
            y: [0, -5, 0],
            scale: [0.78, 0.81, 0.78],
            transition: {
              y: { duration: 3.2, repeat: Infinity, ease: "easeInOut" },
              scale: { duration: 3.2, repeat: Infinity, ease: "easeInOut" },
            },
          },
          /* lifted by the approaching file */
          armed: {
            x: 0,
            y: -9,
            scale: 1.08,
            transition: { type: "spring", stiffness: 380, damping: 20 },
          },
          /* the page dips under the weight of the landing image */
          loaded: {
            x: 0,
            y: [0, 2.5, 0],
            scale: [1, 0.985, 1],
            transition: { duration: 0.4, times: [0, 0.4, 1], ease: "easeOut" },
          },
          /* a small satisfied nod as the check is written */
          nod: {
            x: 0,
            y: 0,
            scale: 1,
            rotate: [0, -1.6, 1.1, 0],
            transition: { duration: 0.65, ease: "easeInOut" },
          },
          /* quiet head-shake */
          invalid: {
            x: [0, -6, 5, -3, 2, 0],
            y: 0,
            scale: 1,
            transition: { duration: 0.38, ease: "easeInOut" },
          },
        }}
        animate={variant}
      >
        {/* the Marker, magnetically attached */}
        <div className="pen" />

        <motion.div
          className="sheet-shadow"
          animate={{ opacity: armed ? 1 : 0 }}
          transition={{ duration: 0.3 }}
        />

        {/* ambient light drifting across the aluminum frame */}
        <div className="sheen" />

        {/* the e-ink screen, inset in the bezel */}
        <div className="screen" data-tauri-drag-region>
        {/* ruled lines: step aside when content arrives, return a beat later */}
        <motion.div
          className="page-lines"
          data-tauri-drag-region
          animate={{ opacity: job || armed || sent ? 0 : 1 }}
          transition={{ duration: 0.35, delay: job || sent ? 0 : 0.25 }}
        >
          <i /><i /><i />
          <span className="brand">reMarkable</span>
        </motion.div>

        {/* the print */}
        <AnimatePresence>
          {job && (
            <motion.div
              key="print"
              className="print"
              initial={{ opacity: 0, scale: 1.14, y: 10, filter: "blur(0px)" }}
              animate={{ opacity: 1, scale: 1, y: 0, filter: "blur(0px)" }}
              /* the ink slowly evaporates off the page */
              exit={{
                opacity: 0,
                scale: 1.04,
                y: -6,
                filter: "blur(7px)",
                transition: { duration: 1.15, ease: [0.4, 0, 0.6, 1] },
              }}
              transition={{
                type: "spring",
                stiffness: 320,
                damping: 18,
                opacity: { duration: 0.2 },
              }}
            >
              {/* color layer beneath */}
              <img src={job.src} alt="" draggable={false} />

              {/* e-ink layer revealed top-to-bottom by the scan */}
              <motion.div
                className="ink"
                initial={{ clipPath: "inset(0 0 100% 0)" }}
                animate={{ clipPath: "inset(0 0 0% 0)" }}
                transition={{ duration: SCAN_MS / 1000, ease: "easeInOut" }}
              >
                <img src={job.src} alt="" draggable={false} />
              </motion.div>

              {/* the scan edge itself */}
              <motion.div
                className="scanline"
                initial={{ top: "0%", opacity: 0.9 }}
                animate={{ top: "100%", opacity: [0.9, 0.9, 0] }}
                transition={{ duration: SCAN_MS / 1000, ease: "easeInOut" }}
              />

              <div className="grain" />
            </motion.div>
          )}
        </AnimatePresence>

        {/* idle doodles, sketched in the corner */}
        <AnimatePresence>
          {doodle !== null && (
            <motion.svg
              key={`doodle-${doodle}`}
              className="doodle"
              viewBox="0 0 24 24"
              fill="none"
              initial={{ opacity: 1 }}
              exit={{ opacity: 0, transition: { duration: 0.9 } }}
            >
              <motion.path
                d={DOODLES[doodle]}
                stroke="rgba(28,28,30,0.4)"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                initial={{ pathLength: 0 }}
                animate={{ pathLength: 1 }}
                transition={{ duration: 1.1, ease: "easeInOut" }}
              />
            </motion.svg>
          )}
        </AnimatePresence>

        {/* the resolution: a check, handwritten by the Marker */}
        <AnimatePresence>
          {sent && (
            <motion.svg
              key="tick"
              className="tick"
              viewBox="0 0 48 44"
              fill="none"
              initial={{ opacity: 1 }}
              exit={{ opacity: 0, transition: { duration: 0.5 } }}
            >
              <motion.path
                d="M11 25 C 15 29, 18 32, 20.5 34.5 C 25 26.5, 32.5 16, 40 10.5"
                stroke="rgba(28,28,30,0.82)"
                strokeWidth="2.6"
                strokeLinecap="round"
                strokeLinejoin="round"
                initial={{ pathLength: 0 }}
                animate={{ pathLength: 1 }}
                transition={{ duration: 0.5, ease: [0.35, 0, 0.3, 1] }}
              />
            </motion.svg>
          )}
        </AnimatePresence>
        </div>
      </motion.div>
      </motion.div>

      {/* quit — only on hover */}
      <AnimatePresence>
        {hover && IS_TAURI && !job && (
          <motion.button
            className="quit"
            onClick={quit}
            title="退出"
            initial={{ opacity: 0, scale: 0.6 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.6 }}
            transition={{ duration: 0.15 }}
          >
            ✕
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}
