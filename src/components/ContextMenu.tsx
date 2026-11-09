import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

export interface MenuEntry {
  label: string;
  icon?: string;
  /** Rendered in red — used for destructive actions. */
  danger?: boolean;
  onSelect: () => void;
}

interface Props {
  x: number;
  y: number;
  items: MenuEntry[];
  onClose: () => void;
}

/** Cursor-anchored menu for the note list.
 *
 * Rendered into `document.body` so the note list's own scroll container cannot
 * clip it, and flipped towards the viewport when the cursor sits near an edge.
 */
export default function ContextMenu({ x, y, items, onClose }: Props) {
  const menu = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y, placed: false });

  useLayoutEffect(() => {
    const el = menu.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    const pad = 6;
    const left = x + width > window.innerWidth - pad ? x - width : x;
    const top = y + height > window.innerHeight - pad ? y - height : y;
    setPos({
      left: Math.max(pad, Math.min(left, window.innerWidth - width - pad)),
      top: Math.max(pad, Math.min(top, window.innerHeight - height - pad)),
      placed: true,
    });
  }, [x, y]);

  useEffect(() => {
    const close = () => onClose();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    // `mousedown` (not click) so the menu disappears before the page reacts.
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    window.addEventListener("keydown", onKey);
    document.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      window.removeEventListener("keydown", onKey);
      document.removeEventListener("scroll", close, true);
    };
  }, [onClose]);

  return createPortal(
    <div
      ref={menu}
      className="context-menu"
      role="menu"
      style={{
        left: pos.left,
        top: pos.top,
        visibility: pos.placed ? "visible" : "hidden",
      }}
      onMouseDown={(e) => e.stopPropagation()}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
      }}
      onKeyDown={(e) => {
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
        e.preventDefault();
        const buttons = Array.from(
          menu.current?.querySelectorAll<HTMLButtonElement>(".context-item") ?? [],
        );
        if (buttons.length === 0) return;
        const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const step = e.key === "ArrowDown" ? 1 : -1;
        const next = (at + step + buttons.length) % buttons.length;
        buttons[next].focus();
      }}
    >
      {items.map((item, index) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          autoFocus={index === 0}
          className={`context-item ${item.danger ? "danger" : ""}`}
          onClick={() => {
            onClose();
            item.onSelect();
          }}
        >
          <span className="context-icon" aria-hidden="true">
            {item.icon ?? ""}
          </span>
          <span>{item.label}</span>
        </button>
      ))}
    </div>,
    document.body,
  );
}
