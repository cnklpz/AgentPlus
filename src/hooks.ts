import { useEffect, useRef } from "react";

/** Open layers (dialogs, palettes) that close on Esc, in the order they opened. */
const layers: { current: () => void }[] = [];

/** A dialog or palette is open: page-level Esc handlers (closing a side panel) leave the key to it. */
export const escapeLayerOpen = () => layers.length > 0;

function onEscape(e: KeyboardEvent) {
  if (e.key !== "Escape" || e.defaultPrevented) return;
  const top = layers[layers.length - 1];
  if (!top) return;
  // Only the topmost layer closes; the ones underneath (and page-level Esc handlers) stay.
  e.preventDefault();
  e.stopImmediatePropagation();
  top.current();
}

/**
 * Esc calls `onClose` — only for the most recently opened layer, so a palette over a
 * dialog closes alone. The listener is registered once; the latest callback is read
 * from a ref, so parents that re-render (and pass a new arrow each time) don't
 * re-run anything else in the dialog. Controls inside that handle Esc themselves
 * (menus, combo boxes) stop it first.
 */
export function useEscape(onClose: () => void) {
  const ref = useRef(onClose);
  ref.current = onClose;
  useEffect(() => {
    const layer = { current: () => ref.current() };
    if (!layers.length) document.addEventListener("keydown", onEscape);
    layers.push(layer);
    return () => {
      const i = layers.indexOf(layer);
      if (i >= 0) layers.splice(i, 1);
      if (!layers.length) document.removeEventListener("keydown", onEscape);
    };
  }, []);
}
