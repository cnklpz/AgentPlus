import { type ReactNode, useLayoutEffect, useRef, useState } from "react";

export interface TabItem<T extends string> {
  id: T;
  label: ReactNode;
  count?: number | null;
}

interface Props<T extends string> {
  items: TabItem<T>[];
  value: T;
  onChange: (id: T) => void;
}

/** Tab strip with an underline that slides to the chosen tab. */
export function TabBar<T extends string>({ items, value, onChange }: Props<T>) {
  const root = useRef<HTMLDivElement>(null);
  const [bar, setBar] = useState<{ left: number; width: number } | null>(null);

  useLayoutEffect(() => {
    const measure = () => {
      const el = root.current?.querySelector<HTMLElement>(`[data-tab="${value}"]`);
      if (el) setBar({ left: el.offsetLeft, width: el.offsetWidth });
    };
    measure();
    // Labels can change width (counts, fonts loading).
    const ro = new ResizeObserver(measure);
    if (root.current) ro.observe(root.current);
    return () => ro.disconnect();
  }, [value, items.length]);

  return (
    <div className="tabs" role="tablist" ref={root}>
      {items.map((t) => (
        <button key={t.id} data-tab={t.id} role="tab" aria-selected={value === t.id} className={`tab${value === t.id ? " on" : ""}`} onClick={() => onChange(t.id)}>
          {t.label}{t.count != null && <span className="tab-count">{t.count}</span>}
        </button>
      ))}
      {bar && <span className="tab-bar" style={{ transform: `translateX(${bar.left}px)`, width: bar.width }} aria-hidden="true" />}
    </div>
  );
}

/**
 * Direction for the tab content's entrance: "fwd" when moving to a tab on the
 * right, "back" when moving left. Pair with `key={value}` on the content.
 */
export function useSlideDir<T extends string>(value: T, order: readonly T[]): "fwd" | "back" {
  const prev = useRef(value);
  const dir = useRef<"fwd" | "back">("fwd");
  if (prev.current !== value) {
    dir.current = order.indexOf(value) >= order.indexOf(prev.current) ? "fwd" : "back";
    prev.current = value;
  }
  return dir.current;
}
