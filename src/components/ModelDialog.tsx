import { useEffect, useRef, useState } from "react";
import { type ModelField, type ModelFieldValue, type ModelGuess, type ModelInput, api } from "../api";
import { type ViewModel, fmtCtx, parseCtx } from "../draft";
import { Dropdown } from "./Dropdown";
import { Modal } from "./Modal";
import { Seg } from "./controls";
import { OptCheck } from "./icons";
import { locale, t, tn, tx } from "../i18n";
import { toggledIn } from "../util";

interface Props {
  /** Agent id (or OpenCode project id), for looking the model up in the catalogs. */
  agent: string;
  agentName: string;
  /** The agent stores a display name per model. */
  hasNames: boolean;
  /** The "name" is really the upstream model id sent in requests (Kimi's `model = …`). */
  nameIsUpstream?: boolean;
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

/** The context window's key among the auto-filled / touched keys (field keys are JSON pointers or names, never this). */
const CTX = "#context";

const lookUp = async (agent: string, id: string): Promise<ModelGuess | null> =>
  (await api.guessModels(agent, [id]).catch(() => ({}) as Record<string, ModelGuess>))[id] ?? null;

/**
 * Adds or edits one model: id, display name, context window, plus the per-model settings
 * the agent declares (input kinds, reasoning, max output…). Unset settings stay out of
 * the config, so the agent's own default applies. A new model's settings are filled in from
 * the model catalogs as its id is typed (like ZCode does); an existing one can ask for it.
 */
export function ModelDialog({ agent, agentName, hasNames, nameIsUpstream = false, hasContext, fields, initial, onSave, onClose }: Props) {
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

  // ZCode matches models against its own rules; an agent with nothing per model has nothing to fill.
  const canGuess = agent !== "zcode" && (hasContext || fields.length > 0);
  const [guess, setGuess] = useState<ModelGuess | null>(null);
  /** Keys whose value came from `guess` and hasn't been edited since. */
  const [auto, setAuto] = useState<Set<string>>(new Set());
  /** Keys the user has edited: a later guess leaves them alone. */
  const touched = useRef(new Set<string>());
  const [matchNote, setMatchNote] = useState<string | null>(null);
  const touch = (k: string) => {
    touched.current.add(k);
    setAuto((a) => (a.has(k) ? new Set([...a].filter((x) => x !== k)) : a));
  };

  /** Puts `g` into the keys `may` allows (clearing them when `g` lacks a value); returns the keys it filled. */
  const fill = (g: ModelGuess | null, may: (k: string) => boolean): Set<string> => {
    const filled = new Set<string>();
    if (hasContext && may(CTX)) {
      setCtx(g?.context ? String(g.context) : "");
      if (g?.context) filled.add(CTX);
    }
    const nv: Values = {};
    const nn: Record<string, string> = {};
    for (const f of fields.filter((f) => may(f.key))) {
      const v = g?.extra[f.key];
      if (f.kind === "number") nn[f.key] = typeof v === "number" ? String(v) : "";
      else nv[f.key] = v;
      if (v !== undefined) filled.add(f.key);
    }
    setVals((x) => ({ ...x, ...nv }));
    setNums((x) => ({ ...x, ...nn }));
    return filled;
  };

  // New model: look the id up as it is typed; a later match replaces what an earlier one filled in.
  useEffect(() => {
    if (initial || !canGuess) return;
    const q = id.trim();
    let live = true;
    const timer = setTimeout(async () => {
      const g = q ? await lookUp(agent, q) : null;
      if (!live) return;
      setGuess(g);
      setAuto(fill(g, (k) => !touched.current.has(k)));
    }, q ? 300 : 0);
    return () => { live = false; clearTimeout(timer); };
  }, [id]); // only a new id starts a lookup

  // Existing model: fill in only what isn't set yet.
  const matchNow = async () => {
    const q = id.trim();
    const g = await lookUp(agent, q);
    setGuess(g);
    if (!g) { setMatchNote(t("modelDialog.matchNone")); return; }
    const unset = (k: string) => {
      if (k === CTX) return ctx.trim() === "";
      const f = fields.find((x) => x.key === k);
      return f?.kind === "number" ? (nums[k] ?? "").trim() === "" : vals[k] === undefined;
    };
    const filled = fill(g, unset);
    setAuto(filled);
    setMatchNote(filled.size ? tn("modelDialog.matchFilled", filled.size, { id: g.matched }) : t("modelDialog.matchNothingNew", { id: g.matched }));
  };

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

  const foot = (
    <>
      <span className="muted tiny grow hint">{t("common.pendingNote")}</span>
      <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={!canSave} onClick={save}>{initial ? t("common.save") : t("common.add")}</button>
    </>
  );
  return (
    <Modal label={initial ? t("modelDialog.editTitle") : t("modelDialog.addTitle")} wide={fields.length > 0} onClose={onClose}
      title={initial ? tx("modelDialog.editHead", { id: <span className="mono">{initial.id}</span> }) : t("modelDialog.addHead", { agent: agentName })}
      onKeyDown={(e) => { if (e.key === "Enter" && (e.target as HTMLElement).tagName === "INPUT") { e.preventDefault(); save(); } }} foot={foot}>
      <div className="field">
        <label htmlFor="md-id">{t("modelDialog.modelId")}</label>
        <input id="md-id" ref={initial ? undefined : first} className="input mono" value={id} disabled={!!initial}
          onChange={(e) => setId(e.target.value)} placeholder={t("modelDialog.modelIdPlaceholder")} />
        {initial && <em className="muted tiny">{t("modelDialog.idLocked")}</em>}
        {initial && canGuess && (
          <div className="row gap6">
            <button type="button" className="link tiny" title={t("modelDialog.matchHint")} onClick={matchNow}>{t("modelDialog.matchButton")}</button>
            {matchNote && <em className="muted tiny">{matchNote}</em>}
          </div>
        )}
        {!initial && guess && (
          <em className="muted tiny">
            {t("modelDialog.matched", { id: guess.matched, source: t(guess.source === "builtin" ? "modelDialog.sourceBuiltin" : "modelDialog.sourceModelsDev") })}
          </em>
        )}
      </div>
      {(hasNames || hasContext) && (
        <div className={hasNames && hasContext ? "form2" : ""}>
          {hasNames && (
            <div className="field">
              <label htmlFor="md-name">{t(nameIsUpstream ? "modelDialog.upstreamModel" : "modelDialog.displayName")}</label>
              <input id="md-name" ref={initial ? first : undefined} className={`input${nameIsUpstream ? " mono" : ""}`} value={name} onChange={(e) => setName(e.target.value)}
                placeholder={t(nameIsUpstream ? "modelDialog.upstreamModelPlaceholder" : "modelDialog.displayNamePlaceholder")} />
            </div>
          )}
          {hasContext && (
            <div className="field">
              <label htmlFor="md-ctx">{t("modelDialog.context")}{auto.has(CTX) && <span className="unsaved">{t("modelDialog.auto")}</span>}</label>
              <input id="md-ctx" ref={initial && !hasNames ? first : undefined} className={`input mono${ctxBad ? " bad" : ""}`} value={ctx}
                onChange={(e) => { touch(CTX); setCtx(e.target.value); }} placeholder={t("modelDialog.contextPlaceholder")} />
              {ctxBad ? <em className="field-err">{t("modelDialog.contextBad")}</em> : ctxVal ? <em className="muted tiny">{t("modelDialog.contextTokens", { n: ctxVal.toLocaleString(locale()), short: fmtCtx(ctxVal) })}</em> : null}
            </div>
          )}
        </div>
      )}

      {groups.map((g) => (
        <section key={g} className="mf-group">
          <div className="mf-title">{g}</div>
          {fields.filter((f) => f.group === g).map((f) => (
            <FieldRow key={f.key} f={f} value={vals[f.key]} text={nums[f.key] ?? ""} bad={badNums.includes(f.key)} auto={auto.has(f.key)}
              changed={f.kind === "number" ? (nums[f.key] ?? "") !== (typeof initial?.extra?.[f.key] === "number" ? String(initial.extra[f.key]) : "") : !same(vals[f.key], initial?.extra?.[f.key])}
              onChange={(v) => { touch(f.key); set(f.key, v); }} onText={(s) => { touch(f.key); setNums((x) => ({ ...x, [f.key]: s })); }} />
          ))}
        </section>
      ))}
      {fields.length > 0 && <em className="muted tiny hint">{t("modelDialog.defaultNote", { agent: agentName })}</em>}
    </Modal>
  );
}

function FieldRow({ f, value, text, bad, auto, changed, onChange, onText }: {
  f: ModelField; value: ModelFieldValue | undefined; text: string; bad: boolean;
  /** The value was filled in from the model catalogs (and not edited since). */
  auto: boolean; changed: boolean;
  onChange: (v: ModelFieldValue | undefined) => void; onText: (s: string) => void;
}) {
  const head = (
    <div className="grow minw0">
      <div className="small strong">
        {f.label}
        {auto ? <span className="unsaved">{t("modelDialog.auto")}</span> : changed && <span className="unsaved">{t("modelDialog.modified")}</span>}
      </div>
      <div className="tiny muted hint">{f.desc}</div>
    </div>
  );

  if (f.kind === "bool") {
    const cur = value === true ? "on" : value === false ? "off" : "";
    return (
      <div className="mf-row">
        {head}
        <Seg className="sm" value={cur} label={f.label} onChange={(k) => onChange(k === "" ? undefined : k === "on")}
          options={[{ value: "", label: t("modelDialog.default") }, { value: "on", label: t("common.yes") }, { value: "off", label: t("common.no") }]} />
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
                const next = toggledIn(start, o);
                onChange(next.length ? next : undefined);
              }}>
              <OptCheck on={on} />
              <span>{i >= 0 ? f.hints[i] || o : o}</span>
              {i >= 0 && f.hints[i] && f.hints[i] !== o && <span className="mono tiny muted">{o}</span>}
            </button>
          );
        })}
      </div>
    </div>
  );
}
