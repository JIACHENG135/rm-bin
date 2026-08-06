/**
 * The paper, once it is out of the tablet.
 *
 * Clicking the device makes the window itself grow upward and the pile fan out
 * of the screen onto the desk. There is no panel and no second window: what
 * you are looking at is the same object, larger.
 *
 * That is why the collapse transform matters more than anything else here.
 * Each card's resting state — when the window is shut — is *exactly* where its
 * sheet sits inside the 112x148 screen: same centre, same angle, same 0.91
 * scale that turns a 68x88 card into the 62x80 sheet the device draws. Nothing
 * fades in and nothing is substituted; one object moves. Get that transform
 * wrong by a few pixels and the whole illusion reads as a popup.
 */

import type { PendingItem } from "./pending";
import {
  CARD,
  CAP,
  COL_STEP,
  FAN_STEP,
  FLOOR,
  GRID_STEP,
  thumbUrl,
  type Geometry,
} from "./pending";

export type Drag = { id: string; dx: number; dy: number };

/** How far a card has to travel before letting go tears it out. */
export const TEAR_PX = 34;
/** Dead zone, so a hover with a twitch in it is not a drag. */
export const DEAD_PX = 6;

type Props = {
  items: PendingItem[];
  geom: Geometry;
  open: boolean;
  hoverId: string | null;
  drag: Drag | null;
  /** ids playing their tear-out exit — still drawn, no longer touchable */
  tearing: string[];
  onCardDown: (id: string, e: React.PointerEvent) => void;
  onCardEnter: (id: string) => void;
  onCardLeave: () => void;
};

export default function Stack({
  items,
  geom,
  open,
  hoverId,
  drag,
  tearing,
  onCardDown,
  onCardEnter,
  onCardLeave,
}: Props) {
  const { fan, cols, shown, gx0, w } = geom;
  const step = fan ? FAN_STEP : GRID_STEP;
  const spread = Math.min(17, 4.2 * Math.min(items.length, 6));

  return (
    <>
      {items.slice(0, CAP).map((it, i) => {
        const col = i % cols;
        const row = Math.floor(i / cols);
        /* the fan bows outward in the middle, the way a splayed deck does */
        const bow = fan
          ? -Math.round(
              Math.sin((i / Math.max(shown - 1, 1)) * Math.PI * 0.62) * 9
            )
          : 0;
        /* Everything is measured from the window's bottom-right corner — the
           one point that never moves. Measuring from the left edge instead
           would make each card's anchor jump the moment the window narrows on
           collapse, and only the transform is animated, so the jump would be
           instant and visible. */
        const right = w - (gx0 + col * COL_STEP + bow) - CARD.w;
        const bottom = FLOOR + row * step;

        /* Where this card has to go to become its sheet again. The device
           centre is 56 from the right edge and 74 up from the bottom, so both
           offsets fall out independent of how wide the window currently is. */
        const toStackX = right + CARD.w / 2 - 56;
        const toStackY = bottom + CARD.h / 2 - 74;
        const fanRot =
          items.length <= 1
            ? 0
            : (i / (Math.min(items.length, 6) - 1 || 1) - 0.5) * spread;
        /* laid out: the fan leans progressively, the grid keeps a little
           residual scatter so it never looks like a spreadsheet */
        const rest = fan ? -Math.min(7, i * 1.2) : (((i * 37) % 7) - 3) * 0.5;

        const d = drag && drag.id === it.id ? drag : null;
        const dist = d ? Math.hypot(d.dx, d.dy) : 0;
        const dragRot = d ? Math.max(-14, Math.min(14, d.dx * 0.35)) : 0;

        let transform: string;
        let transition: string;
        let delay: string;
        if (d) {
          transform = `translate(${d.dx.toFixed(1)}px, ${d.dy.toFixed(
            1
          )}px) scale(1.04) rotate(${dragRot.toFixed(1)}deg)`;
          transition = "none";
          delay = "0ms";
        } else if (open) {
          transform = `translate(0px, 0px) scale(1) rotate(${rest.toFixed(
            1
          )}deg)`;
          transition =
            "transform .42s cubic-bezier(.22,1.10,.36,1), opacity .3s ease-out";
          /* bottom card first: the pile opens from where your eye already is */
          delay = `${i * 26}ms`;
        } else {
          transform = `translate(${toStackX.toFixed(1)}px, ${toStackY.toFixed(
            1
          )}px) scale(.91) rotate(${fanRot.toFixed(1)}deg)`;
          transition =
            "transform .3s cubic-bezier(.4,0,.6,1), opacity .22s ease-out";
          /* closing runs the other way — top card first, like being gathered */
          delay = `${(shown - 1 - i) * 18}ms`;
        }

        const torn = tearing.includes(it.id);
        const lifted = hoverId === it.id && !torn;
        return (
          <div
            key={it.id}
            className="card"
            style={{
              right: `${right}px`,
              bottom: `${bottom}px`,
              /* torn: it evaporates from exactly where it was lying, on the
                 same curve the ink uses to leave the screen */
              transform: torn
                ? `translate(0px, -14px) scale(1) rotate(${rest.toFixed(1)}deg)`
                : transform,
              transition: torn
                ? "transform 1.15s cubic-bezier(.4,0,.6,1), opacity 1.15s cubic-bezier(.4,0,.6,1), filter 1.15s cubic-bezier(.4,0,.6,1)"
                : transition,
              transitionDelay: torn ? "0ms" : delay,
              filter: torn ? "blur(7px)" : undefined,
              zIndex: d ? 40 : lifted ? 35 : open && fan ? 10 + (shown - 1 - i) : 10 + i,
              /* past the threshold it goes pale: let go and it is gone */
              opacity: torn ? 0 : open ? (dist > TEAR_PX ? 0.5 : 1) : 0,
              pointerEvents: open && !torn ? "auto" : "none",
            }}
            onPointerDown={(e) => !torn && onCardDown(it.id, e)}
            onMouseEnter={() => onCardEnter(it.id)}
            onMouseLeave={onCardLeave}
          >
            <div className="card-paper" data-lifted={lifted}>
              <div className="card-bar" style={{ background: it.tint }} />
              <img src={thumbUrl(it.thumb)} alt="" draggable={false} />
              <div className="card-tear" />
            </div>
            {/* the name is not worth permanent room: it appears under the
                pointer and leaves with it */}
            <div className="card-label" data-lifted={lifted}>
              {it.name}
            </div>
          </div>
        );
      })}
    </>
  );
}
