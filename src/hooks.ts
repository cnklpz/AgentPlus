import { useEffect, useRef } from "react";

/**
 * Esc calls `onClose`. The listener is registered once; the latest callback is read
 * from a ref, so parents that re-render (and pass a new arrow each time) don't
 * re-run anything else in the dialog.
 */
export function useEscape(onClose: () => void) {
  const ref = useRef(onClose);
  ref.current = onClose;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") ref.current(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);
}
