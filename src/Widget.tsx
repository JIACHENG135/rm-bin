/**
 * The 112x148 device, and the stack of paper sitting on it.
 *
 * This panel never changes size and never grows a control. Everything it has
 * to say — how much is waiting, how long it has waited, whether the tablet is
 * there, how far along a send is — it says by how the paper sits. That is the
 * whole constraint: at this size a label is a smudge, but a stack that is
 * visibly heavier is legible from across the room.
 */

import type { CSSProperties } from "react";
import type { PendingItem, Placed } from "./pending";
import { thumbUrl } from "./pending";

export type Phase =
  | "idle"
  | "pending"
  | "armed"
  | "charging"
  /** A: this machine is building the documents */
  | "render"
  /** the stack lifting off, and the frame springing back light */
  | "drain"
  /** B: the tablet is dark and restarting */
  | "restart"
  /** the e-ink panel settling */
  | "flash"
  | "done"
  | "invalid";

type Props = {
  items: PendingItem[];
  phase: Phase;
  /** 0..1 — the long press while charging, the render progress during A */
  charge: number;
  online: boolean;
  hover: boolean;
  /** the last document of a delivered batch: one line is all that fits */
  receipt: Placed | null;
  /** true once the queue has been waiting longer than three days */
  settled: boolean;
  /** index into DOODLES, or null */
  doodle: number | null;
  /** the window is open and the paper is out on the desk instead */
  hideStack: boolean;
};

/* little idle doodles: star, wave, spiral */
export const DOODLES = [
  "M12 3.5 L14.3 9.3 L20.5 9.3 L15.5 13 L17.4 19 L12 15.4 L6.6 19 L8.5 13 L3.5 9.3 L9.7 9.3 Z",
  "M3 13 C6 9, 9 9, 12 13 C15 17, 18 17, 21 13",
  "M12.5 12.5 C13.8 12.3 14.2 10.9 13 10.3 C11.2 9.5 9.4 11 9.8 12.9 C10.3 15.4 13.2 16.5 15.4 15.2 C18.2 13.6 18.7 9.8 16.4 7.6",
] as const;

/** Six sheets is as many as the eye counts without counting. */
const FACES = 6;
/** Past twelve the difference stops being visible, so it stops being modelled. */
const CAP = 12;

export default function Widget({
  items,
  phase,
  charge,
  online,
  hover,
  receipt,
  settled,
  doodle,
  hideStack,
}: Props) {
  const n = items.length;
  const k = Math.min(n, CAP);

  /* ————— posture, entirely a function of load —————
     Weight said three ways at once, because any one of them alone reads as a
     glitch: it sits lower, it leans further forward, and its shadow moves
     further from the desk. Nothing travels more than 8px.

     Three days of waiting adds a little more of all three — the single,
     never-repeated settle. It used to be carried mostly by the breathing;
     with that gone it has to live in the sink and the shadow instead. */
  const sink = k * 0.55 + (settled ? 1.4 : 0);
  const lean = k * 0.25;
  const s0 = n ? 0.96 - k * 0.002 : 0.78;
  const shY = 4 + k * 0.58 + (settled ? 2 : 0);
  const shB = 9 + k * 0.92;
  const shA = 0.16 + k * 0.0117;

  const armed = phase === "armed";
  const charging = phase === "charging";
  const rendering = phase === "render";
  const draining = phase === "drain";
  const restart = phase === "restart";
  const flash = phase === "flash";
  const done = phase === "done";
  const invalid = phase === "invalid";
  const busy = rendering || draining || restart || flash;

  /* During A the frame carries on breathing: the device has not been touched
     yet, and pretending otherwise would spend the stillness too early. */
  const motion = armed
    ? "armed"
    : charging
    ? "charging"
    : invalid
    ? "invalid"
    : draining
    ? "rebound"
    : restart || flash || done
    ? "busy"
    : "rest";

  const style = {
    "--y0": `${sink.toFixed(2)}px`,
    "--lean": `${lean.toFixed(2)}deg`,
    "--s0": s0.toFixed(3),
    "--press": `${(charge * 2.4).toFixed(2)}px`,
    "--pressScale": (s0 - charge * 0.05).toFixed(3),
    "--busyScale": restart ? 0.985 : 1,
    boxShadow: `0 1px 2px rgba(0,0,0,.14), 0 ${
      armed ? 14 : shY
    }px ${armed ? 24 : shB}px rgba(0,0,0,${armed ? 0.24 : shA})`,
  } as CSSProperties;

  /* the fan: the newest sheets on top, each showing a corner of the one below */
  const m = Math.min(n, FACES);
  const faces = items.slice(-m);
  const spread = m <= 1 ? 0 : Math.min(17, 4.2 * m);
  /* the colour draining out as the picture is pressed into a document */
  const drain = rendering ? Math.min(1, charge) : draining ? 1 : 0;
  const extra = Math.max(0, Math.min(n - m, FACES));

  /* the screen dims *while you are still pressing*, so you see what you are
     about to cause before you have committed to it */
  const dim = charging ? charge * 0.4 : restart || flash ? 1 : 0;

  return (
    <div
      className="fade"
      style={{
        opacity: armed || invalid || hover || busy || done || n ? 1 : 0.5,
      }}
    >
      <div className="sheet" data-motion={motion} style={style}>
        <div className="pen" />
        <div className="sheen" />

        <div className="screen">
          {/* ruled lines: step aside when the stack arrives */}
          <div
            className="page-lines"
            style={{ opacity: n || armed || busy || done ? 0 : 1 }}
          >
            <i />
            <i />
            <i />
            <span className="brand">reMarkable</span>
          </div>

          {/* past six, sheets stop fanning and start being edges — thickness
              without width, so twelve is a pile and not a hand of cards */}
          {Array.from({ length: extra }, (_, i) => (
            <div
              key={`sliver-${i}`}
              className="sliver"
              style={{
                transform: `translateY(${(
                  -(m - 1) * 0.85 -
                  (i + 1) * 0.45 +
                  1.5
                ).toFixed(2)}px) rotate(${(
                  (i % 2 ? -1 : 1) *
                  (2 + i * 0.6)
                ).toFixed(2)}deg)`,
                opacity: hideStack ? 0 : 1,
                transitionDelay: hideStack ? "0ms" : "120ms",
              }}
            />
          ))}

          {faces.map((it, j) => {
            const t = m === 1 ? 0.5 : j / (m - 1);
            const top = j === m - 1;
            const rot = (t - 0.5) * spread * (top ? 0.45 : 1);
            const depth = m - 1 - j;
            return (
              <div
                key={it.id}
                className="face"
                style={{
                  zIndex: 2 + j,
                  transform: draining
                    ? `translateY(-14px) rotate(${rot.toFixed(2)}deg)`
                    : `translateY(${(-depth * 0.85 + 1.5).toFixed(
                        2
                      )}px) rotate(${rot.toFixed(2)}deg)`,
                  filter: `grayscale(${drain}) brightness(${(
                    1 -
                    depth * 0.055
                  ).toFixed(3)}) blur(${draining ? 7 : 0}px)`,
                  opacity: draining || hideStack ? 0 : 1,
                  transitionDelay: draining
                    ? undefined
                    : hideStack
                    ? "0ms"
                    : "120ms",
                  /* the top sheet goes first, the way a hand takes them */
                  transition: draining
                    ? `transform 1.15s cubic-bezier(.4,0,.6,1) ${
                        depth * 0.06
                      }s, opacity 1.15s cubic-bezier(.4,0,.6,1) ${
                        depth * 0.06
                      }s, filter 1.15s cubic-bezier(.4,0,.6,1) ${depth * 0.06}s`
                    : undefined,
                }}
              >
                <div
                  className="face-inner"
                  style={{ background: it.tint || "#fbfaf7" }}
                >
                  <img src={thumbUrl(it.thumb)} alt="" draggable={false} />
                  <div className="grain" />
                </div>
              </div>
            );
          })}

          {rendering && <div className="sweep" />}

          {/* the crop mark closing: how much longer, and nothing else */}
          <svg
            className="ring"
            viewBox="0 0 80 106"
            fill="none"
            style={{ opacity: charging ? 1 : 0 }}
          >
            <rect
              x="1.6"
              y="1.6"
              width="76.8"
              height="102.8"
              rx="2.4"
              stroke="rgba(28,28,30,.14)"
              strokeWidth="1.4"
            />
            <rect
              className={`ring-lit${charge >= 1 ? " ring-ready" : ""}`}
              x="1.6"
              y="1.6"
              width="76.8"
              height="102.8"
              rx="2.4"
              stroke="#0088b0"
              strokeWidth="1.4"
              strokeLinecap="round"
              pathLength={100}
              strokeDasharray={100}
              strokeDashoffset={100 - charge * 100}
            />
          </svg>

          <div
            className={`blackout${flash ? " eink" : ""}`}
            style={{
              opacity: dim,
              transition: charging
                ? "opacity .12s linear"
                : "opacity .5s cubic-bezier(.4,0,.6,1)",
            }}
          >
            {restart && <div className="blackout-mark" />}
          </div>

          {/* the check, written by the Marker */}
          <svg
            className="tick"
            viewBox="0 0 48 44"
            fill="none"
            style={{ opacity: done ? 1 : 0, top: done && receipt ? "34%" : "50%" }}
          >
            <path
              d="M11 25 C 15 29, 18 32, 20.5 34.5 C 25 26.5, 32.5 16, 40 10.5"
              stroke="rgba(28,28,30,.82)"
              strokeWidth="2.6"
              strokeLinecap="round"
              strokeLinejoin="round"
              pathLength={100}
              strokeDasharray={100}
              strokeDashoffset={done ? 0 : 100}
              style={{
                transition: done
                  ? "stroke-dashoffset .5s cubic-bezier(.35,0,.3,1)"
                  : "none",
              }}
            />
          </svg>

          <div
            className="receipt"
            style={{ opacity: done && receipt ? 1 : 0 }}
          >
            <div className="receipt-folder">{receipt?.folder ?? ""}</div>
            <div className="receipt-name">{receipt?.name ?? ""}</div>
          </div>

          <div
            className="count"
            style={{ opacity: n > 1 && !busy && !done && !hideStack ? 1 : 0 }}
          >
            {n}
          </div>

          <div
            className="dot"
            data-online={online}
            style={{ opacity: busy ? 0 : 1 }}
          />

          {doodle !== null && (
            <svg className="doodle" viewBox="0 0 24 24" fill="none">
              <path
                d={DOODLES[doodle]}
                stroke="rgba(28,28,30,.4)"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                pathLength={100}
                strokeDasharray={100}
                strokeDashoffset={100}
                style={{ animation: "rmb-draw 1.1s ease-in-out forwards" }}
              />
            </svg>
          )}
        </div>
      </div>
    </div>
  );
}
