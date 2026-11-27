// Marks <html data-idle> while the window is hidden or not in front, so styles can stop
// decorative loops (like the gateway's pulsing dot) that would otherwise keep the
// compositor busy all day in the background.

type Target = Pick<EventTarget, "addEventListener" | "removeEventListener">;
type Doc = Target & { hidden: boolean; hasFocus(): boolean; documentElement: { dataset: DOMStringMap } };

/** Whether the window counts as idle: hidden, or shown but without focus. */
export const isIdle = (doc: Pick<Doc, "hidden" | "hasFocus">) => doc.hidden || !doc.hasFocus();

/** Keeps `data-idle` in sync with the window state; returns a function that stops it. */
export function installIdle(win: Target = window, doc: Doc = document): () => void {
  const sync = () => {
    if (isIdle(doc)) doc.documentElement.dataset.idle = "";
    else delete doc.documentElement.dataset.idle;
  };
  sync();
  win.addEventListener("focus", sync);
  win.addEventListener("blur", sync);
  doc.addEventListener("visibilitychange", sync);
  return () => {
    win.removeEventListener("focus", sync);
    win.removeEventListener("blur", sync);
    doc.removeEventListener("visibilitychange", sync);
    delete doc.documentElement.dataset.idle;
  };
}
