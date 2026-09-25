import { useState } from "react";
import { t } from "../i18n";
import { toggledIn } from "../util";

/** `pool` plus the ids of `ids` it doesn't list yet (the same array when nothing is new). */
export function growPool(pool: readonly string[], ids: readonly string[]): string[] {
  const more = [...new Set(ids)].filter((id) => !pool.includes(id));
  return more.length ? [...pool, ...more] : (pool as string[]);
}

/**
 * Models a dialog has listed so far. It only grows: unticking a model (or "select none") must
 * not drop it from the list, or it could only come back by typing it in again. `reset` starts
 * over (another vendor template was picked: the previous one's models don't belong to it).
 */
export function useModelPool(initial: () => string[]): [string[], (ids: readonly string[]) => void, (ids: readonly string[]) => void] {
  const [pool, setPool] = useState<string[]>(() => growPool([], initial()));
  const add = (ids: readonly string[]) => setPool((p) => growPool(p, ids));
  const reset = (ids: readonly string[]) => setPool(growPool([], ids));
  return [pool, add, reset];
}

interface Props {
  /** Every model listed (see useModelPool); ticked ones outside it are listed too. */
  pool: string[];
  checked: string[];
  onChange: (ids: string[]) => void;
  /** Model ids typed in by hand: add them to the pool and tick them. */
  onAdd: (ids: string[]) => void;
  /** Shown when nothing is listed. */
  empty: string;
  /** Marks a model as new (fetched, not in the provider yet). */
  isNew?: (id: string) => boolean;
  /** Count, filter (long lists) and select all / none above the list. */
  bar?: boolean;
}

/** Checkbox list of model ids, plus a field to add ids by hand. */
export function ModelPicker({ pool, checked, onChange, onAdd, empty, isNew, bar }: Props) {
  const [filter, setFilter] = useState("");
  const [manual, setManual] = useState("");
  const all = [...new Set([...pool, ...checked])];
  const q = filter.trim().toLowerCase();
  const shown = q ? all.filter((m) => m.toLowerCase().includes(q)) : all;
  const addManual = () => {
    const ids = manual.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
    if (ids.length) onAdd(ids);
    setManual("");
  };
  const allOn = checked.length === all.length;
  return (
    <>
      {bar && (
        <div className="mpick-bar">
          <span className="tiny muted">{t("modelPicker.selectedN", { n: checked.length })}</span>
          {all.length > 8 && <input className="input mono mpick-filter" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder={t("common.filter")} />}
          <span className="grow" />
          {all.length > 0 && <button type="button" className="link tiny" onClick={() => onChange(allOn ? [] : all)}>{allOn ? t("common.selectNone") : t("common.selectAll")}</button>}
        </div>
      )}
      <div className="pick-list wide">
        {all.length === 0 && <div className="muted small">{empty}</div>}
        {shown.map((m) => (
          <label key={m} className="pick">
            <input type="checkbox" checked={checked.includes(m)} onChange={() => onChange(toggledIn(checked, m))} />
            <span className="mono small">{m}</span>
            {isNew?.(m) && <span className="mtag new">{t("common.tagNew")}</span>}
          </label>
        ))}
      </div>
      <div className="row gap6">
        <input className="input mono grow" value={manual} onChange={(e) => setManual(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); addManual(); } }} placeholder={t("modelPicker.manualPlaceholder")} />
        <button type="button" className="btn" disabled={!manual.trim()} onClick={addManual}>{t("common.add")}</button>
      </div>
    </>
  );
}
