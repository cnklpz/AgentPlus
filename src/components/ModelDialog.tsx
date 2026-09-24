import { useEffect, useRef, useState } from "react";
import { useEscape } from "../hooks";
import type { ModelField, ModelFieldValue, ModelInput } from "../api";
import { type ViewModel, fmtCtx, parseCtx } from "../draft";
import { Dropdown } from "./Dropdown";
import { Icon } from "./icons";
import { locale, t, tx } from "../i18n";

interface Props {
  agentName: string;
  /** The agent stores a display name per model. */
  hasNames: boolean;
  /** The agent stores a context window per model. */
  hasContext: boolean;
  fields: ModelField[];
  /** Model being edited (with pending edits); undefined = add a new one. */
  initial?: ViewModel;
  onSave: (m: ModelInput) => void;
  onClose: () => void;
}

type Values = Record<string, ModelFieldValue | undefined>;

const same = (a: ModelFieldValue | undefined, b: ModelFieldValue | undefined) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

/**
 * Adds or edits one model: id, display name, context window, plus the per-model settings
 * the agent declares (input kinds, reasoning, max output…). Unset settings stay out of
 * the config, so the agent's own default applies.
 */
export function ModelDialog({ agentName, hasNames, hasContext, fields, initial, onSave, onClose }: Props) {
  const [id, setId] = useState(initial?.id ?? "");
  const [name, setName] = useState(initial?.name ?? "");
  const [ctx, setCtx] = useState(initial?.context ? String(initial.context) : "");
  const [vals, setVals] = useState<Values>(() => ({ ...(initial?.extra ?? {}) }));
  // Numbers are typed as text ("8k") and parsed on save.
  const [nums, setNums] = useState<Record<string, string>>(() =>
    Object.fromEntries(fields.filter((f) => f.kind === "number" && typeof initial?.extra?.[f.key] === "number").map((f) => [f.key, String(initial!.extra![f.key])])),
  );
  const first = useRef<HTMLInputElement>(null);
  useEffect(() => { first.current?.focus(); }, []);
  useEscape(onClose);

  const ctxVal = parseCtx(ctx);
  const ctxBad = ctx.trim() !== "" && ctxVal === null;
  const badNums = fields.filter((f) => f.kind === "number" && (nums[f.key] ?? "").trim() !== "" && parseCtx(nums[f.key]) === null).map((f) => f.key);
  const canSave = id.trim() !== "" && !ctxBad && badNums.length === 0;

  const set = (k: string, v: ModelFieldValue | undefined) => setVals((x) => ({ ...x, [k]: v }));

  const save = () => {
    if (!canSave) return;
    const cur: Values = { ...vals };
    for (const f of fields.filter((f) => f.kind === "number")) {
      const s = (nums[f.key] ?? "").trim();
      cur[f.key] = s ? parseCtx(s) ?? undefined : undefined;
    }
    // Only what changed; null clears a setting.
    const extra: Record<string, ModelFieldValue | null> = {};
    for (const f of fields) {
      const was = initial?.extra?.[f.key];
      if (!same(cur[f.key], was)) extra[f.key] = cur[f.key] ?? null;
    }
    onSave({ id: id.trim(), name: hasNames && name.trim() ? name.trim() : null, context: hasContext ? ctxVal : null, extra });
  };

  const groups = [...new Set(fields.map((f) => f.group))];

  return (
    <div className="modal-bg" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className={`modal${fields.length ? " wide" : ""}`} role="dialog" aria-modal="true" aria-label={initial ? t("modelDialog.editTitle") : t("modelDialog.addTitle")}
        onKeyDown={(e) => { if (e.key === "Enter" && (e.target as HTMLElement).tagName === "INPUT") { e.preventDefault(); save(); } }}>
        <div className="modal-head">
          <h2>{initial ? tx("modelDialog.editHead", { id: <span className="mono">{initial.id}</span> }) : t("modelDialog.addHead", { agent: agentName })}</h2>
          <button className="icon-btn" aria-label={t("common.close")} onClick={onClose}><Icon.close /></button>
        </div>

        <div className="modal-body">
          <div className="field">
            <label htmlFor="md-id">{t("modelDialog.modelId")}</label>
            <input id="md-id" ref={initial ? undefined : first} className="input mono" value={id} disabled={!!initial}
              onChange={(e) => setId(e.target.value)} placeholder={t("modelDialog.modelIdPlaceholder")} />
            {initial && <em className="muted tiny">{t("modelDialog.idLocked")}</em>}
          </div>
          {(hasNames || hasContext) && (
            <div className={hasNames && hasContext ? "form2" : ""}>
              {hasNames && (
                <div className="field">
                  <label htmlFor="md-name">{t("modelDialog.displayName")}</label>
                  <input id="md-name" ref={initial ? first : undefined} className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("modelDialog.displayNamePlaceholder")} />
                </div>
              )}
              {hasContext && (
                <div className="field">
                  <label htmlFor="md-ctx">{t("modelDialog.context")}</label>
                  <input id="md-ctx" ref={initial && !hasNames ? first : undefined} className={`input mono${ctxBad ? " bad" : ""}`} value={ctx}
                    onChange={(e) => setCtx(e.target.value)} placeholder={t("modelDialog.contextPlaceholder")} />
                  {ctxBad ? <em className="field-err">{t("modelDialog.contextBad")}</em> : ctxVal ? <em className="muted tiny">{t("modelDialog.contextTokens", { n: ctxVal.toLocaleString(locale()), short: fmtCtx(ctxVal) })}</em> : null}
                </div>
              )}
            </div>
          )}

          {groups.map((g) => (
            <section key={g} className="mf-group">
              <div className="mf-title">{g}</div>
              {fields.filter((f) => f.group === g).map((f) => (
                <FieldRow key={f.key} f={f} value={vals[f.key]} text={nums[f.key] ?? ""} bad={badNums.includes(f.key)}
                  changed={f.kind === "number" ? (nums[f.key] ?? "") !== (typeof initial?.extra?.[f.key] === "number" ? String(initial.extra[f.key]) : "") : !same(vals[f.key], initial?.extra?.[f.key])}
                  onChange={(v) => set(f.key, v)} onText={(s) => setNums((x) => ({ ...x, [f.key]: s }))} />
              ))}
            </section>
          ))}
          {fields.length > 0 && <em className="muted tiny">{t("modelDialog.defaultNote", { agent: agentName })}</em>}
        </div>

        <div className="modal-foot">
          <span className="muted tiny grow">{t("modelDialog.pendingNote")}</span>
          <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
          <button className="btn primary" disabled={!canSave} onClick={save}>{initial ? t("common.save") : t("common.add")}</button>
        </div>
      </div>
    </div>
  );
}

function FieldRow({ f, value, text, bad, changed, onChange, onText }: {
  f: ModelField; value: ModelFieldValue | undefined; text: string; bad: boolean; changed: boolean;
  onChange: (v: ModelFieldValue | undefined) => void; onText: (s: string) => void;
}) {
  const head = (
    <div className="grow minw0">
      <div className="small strong">{f.label}{changed && <span className="unsaved">{t("modelDialog.modified")}</span>}</div>
      <div className="tiny muted">{f.desc}</div>
    </div>
  );

  if (f.kind === "bool") {
    const cur = value === true ? "on" : value === false ? "off" : "";
    return (
      <div className="mf-row">
        {head}
        <div className="seg sm" role="radiogroup" aria-label={f.label}>
          {([["", t("modelDialog.default")], ["on", t("common.yes")], ["off", t("common.no")]] as const).map(([k, l]) => (
            <button key={k} type="button" role="radio" aria-checked={cur === k} className={cur === k ? "on" : ""}
              onClick={() => onChange(k === "" ? undefined : k === "on")}>{l}</button>
          ))}
        </div>
      </div>
    );
  }

  if (f.kind === "number") {
    return (
      <div className="mf-row">
        {head}
        <div className="mf-num">
          <input className={`input mono${bad ? " bad" : ""}`} value={text} aria-label={f.label} placeholder={t("modelDialog.default")} onChange={(e) => onText(e.target.value)} />
          {bad ? <em className="field-err">{t("modelDialog.numberBad")}</em> : parseCtx(text) ? <em className="muted tiny">{parseCtx(text)!.toLocaleString(locale())}</em> : null}
        </div>
      </div>
    );
  }

  if (f.kind === "select") {
    return (
      <div className="mf-row">
        {head}
        <div className="mf-sel">
          <Dropdown value={typeof value === "string" ? value : ""} label={f.label} onChange={(v) => onChange(v || undefined)}
            options={[{ value: "", label: t("modelDialog.default") }, ...f.options.map((o, i) => ({ value: o, label: f.hints[i] || o }))]} />
        </div>
      </div>
    );
  }

  // chips: several of the options; unset = default.
  const arr = Array.isArray(value) ? value : null;
  const extra = arr?.filter((v) => !f.options.includes(v)) ?? [];
  return (
    <div className="mf-row stacked">
      <div className="row between">
        {head}
        {arr ? <button type="button" className="link tiny" onClick={() => onChange(undefined)}>{t("modelDialog.resetDefault")}</button> : <span className="tiny muted">{t("modelDialog.default")}</span>}
      </div>
      <div className="mf-chips">
        {[...f.options, ...extra].map((o) => {
          const i = f.options.indexOf(o);
          const on = !!arr?.includes(o);
          return (
            <button key={o} type="button" className={`mf-chip${on ? " on" : ""}`} aria-pressed={on}
              onClick={() => {
                // Nothing picked means "not set", not an empty list.
                // Starting from the default, keep text in the list: "image" alone would drop it.
                const start = arr ?? (f.options.includes("text") && o !== "text" ? ["text"] : []);
                const next = on ? start.filter((x) => x !== o) : [...start, o];
                onChange(next.length ? next : undefined);
              }}>
              <span className="opt-check" aria-hidden="true">
                {on && <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="#fff" strokeWidth="3.5" strokeLinecap="round" strokeLinejoin="round"><path d="M20 6 9 17l-5-5" /></svg>}
              </span>
              <span>{i >= 0 ? f.hints[i] || o : o}</span>
              {i >= 0 && f.hints[i] && f.hints[i] !== o && <span className="mono tiny muted">{o}</span>}
            </button>
          );
        })}
      </div>
    </div>
  );
}
