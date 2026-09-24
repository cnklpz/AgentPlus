import type { KeyboardEvent, MouseEvent } from "react";
import type { Provider } from "../api";
import type { ViewProvider } from "../draft";
import { t, tn } from "../i18n";
import { scrub, scrubHost } from "../privacy";

export type Latency = number | "pending" | string | undefined;

const PALETTE = ["#0F766E", "#7C3AED", "#1F2937", "#C2410C", "#2F54EB", "#B45309", "#0E7490", "#9D174D"];

/** "host:port/path" → "host:port"; used to spot the same service across agents. */
export function serviceKey(p: Provider): string {
  return p.baseUrl ? p.host.split("/")[0] : "";
}

/** Same service host → same avatar color, so duplicates across agents read as one. */
export function colorFor(p: Provider): string {
  if (p.builtin) return "var(--avatar-builtin)";
  const key = serviceKey(p) || p.id;
  let h = 0;
  for (const c of key) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return PALETTE[h % PALETTE.length];
}

export function initials(name: string): string {
  const w = name.replace(/[^A-Za-z0-9一-龥 ]/g, " ").trim();
  const digits = w.match(/^\d{1,3}/);
  if (digits) return digits[0];
  if (/^[A-Za-z]/.test(w)) return w.slice(0, w.length > 1 && /^[A-Z]{2}/.test(w) ? 2 : 1).toUpperCase();
  return w.slice(0, 1) || "?";
}

/** Latency text + signal level (0–3) for a provider. */
export function latencyView(p: Provider, off: boolean, latency: Latency): { text: string; level: number; live: boolean } {
  const live = typeof latency === "number" && p.compatible && !off;
  const level = !live ? 0 : (latency as number) < 200 ? 3 : (latency as number) < 400 ? 2 : 1;
  let text: string;
  if (!p.compatible) text = scrub(p.reason) ?? t("providerCard.incompatible");
  else if (off) text = t("providerCard.offNoTest");
  else if (!p.baseUrl) text = t("providerCard.accountLogin");
  else if (latency === "pending") text = t("providerCard.testing");
  else if (typeof latency === "number") text = `${latency} ms`;
  else text = scrub(latency) ?? t("providerCard.untested");
  return { text, level, live };
}

export function Bars({ level }: { level: number }) {
  return (
    <span className="bars" aria-hidden="true">
      {[5, 8, 12].map((h, i) => (
        <span key={h} style={{ height: h, background: i < level ? (level >= 2 ? "#16A34A" : "#D97706") : "var(--line-dash)" }} />
      ))}
    </span>
  );
}

interface Props {
  p: ViewProvider;
  mode: "single" | "multi";
  isCurrent: boolean;
  /** Current only in the draft: the switch hasn't been written yet. */
  switching: boolean;
  selected: boolean;
  enabled: boolean;
  visible: number;
  latency: Latency;
  readonly: boolean;
  onSelect: () => void;
  onAction: () => void;
  onModels: () => void;
  /** Re-measure latency (click on the latency). */
  onTest: () => void;
}

export function ProviderCard({ p, mode, isCurrent, switching, selected, enabled, visible, latency, readonly, onSelect, onAction, onModels, onTest }: Props) {
  const off = mode === "multi" && !enabled;
  let tag: { text: string; cls: string } | null = null;
  if (p.isDeleted) tag = { text: t("providerCard.tagDeleting"), cls: "tag-del" };
  else if (p.isNew) tag = { text: t("providerCard.tagNew"), cls: "tag-new" };
  else if (!p.compatible) tag = { text: t("providerCard.incompatible"), cls: "tag-muted" };
  else if (off) tag = { text: t("common.disabled"), cls: "tag-muted" };
  else if (isCurrent && switching) tag = { text: t("providerCard.tagSwitching"), cls: "tag-new" };
  else if (isCurrent) tag = { text: t("providerCard.tagCurrent"), cls: "tag-accent" };
  else if (p.builtin) tag = { text: t("providerCard.tagBuiltin"), cls: "tag-soft" };

  const lat = latencyView(p, off, latency);
  const total = p.models.length;
  const stop = (fn: () => void) => (e: MouseEvent) => { e.stopPropagation(); fn(); };
  // Only the card itself: Enter / Space on a button inside it activates that button.
  const onKey = (e: KeyboardEvent) => { if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); onSelect(); } };

  const pending = p.isNew || p.isDeleted;
  return (
    <div
      className={`pcard${isCurrent ? " current" : ""}${selected ? " selected" : ""}${off || !p.compatible || p.isDeleted ? " dim" : ""}${p.isNew ? " fresh" : ""}`}
      role="button"
      tabIndex={0}
      aria-pressed={selected}
      aria-label={t("providerCard.ariaDetails", { name: p.name })}
      data-url={p.baseUrl ?? undefined}
      data-ctx="provider"
      data-pid={p.id}
      onClick={onSelect}
      onKeyDown={onKey}
    >
      <div className="pcard-head">
        <span className="pavatar" style={{ background: colorFor(p) }}>{initials(p.name)}</span>
        <span className="pcard-title">
          <span className="pcard-name">
            <span className="ellipsis">{p.name}</span>
            {tag && <span className={`ptag ${tag.cls}`}>{tag.text}</span>}
          </span>
          <span className="pcard-host mono ellipsis">{scrubHost(p.host) || "—"}</span>
        </span>
      </div>
      <div className="pcard-meta">
        {p.apis.map((a) => <span key={a} className="api-chip">{a}</span>)}
        {p.officialAuth && <span className="api-chip" title={t("providerCard.officialAuthHint")}>{t("providerCard.officialAuth")}</span>}
        {p.isEdited && !p.isDeleted && <span className="api-chip edited">{t("providerCard.edited")}</span>}
        {total > 0 && p.compatible && !pending && (
          <button className="link" onClick={stop(onModels)}>{tn("providerCard.visibleModels", total, { visible })}</button>
        )}
        {p.isNew && <span className="tiny muted">{tn("providerCard.modelCount", total)}</span>}
      </div>
      <div className="pcard-foot">
        {p.baseUrl && p.compatible && !off && !pending ? (
          <button className="lat-btn" title={t("providerCard.retestHint")} onClick={stop(onTest)} disabled={latency === "pending"}>
            <Bars level={lat.level} />
            <span className={`lat${lat.live ? (lat.level >= 2 ? " good" : " slow") : ""}`}>{lat.text}</span>
            <span className="lat-re" aria-hidden="true">↻</span>
          </button>
        ) : (
          <>
            <Bars level={0} />
            <span className="lat">{pending ? (p.isNew ? t("providerCard.afterApply") : t("providerCard.deleteOnApply")) : lat.text}</span>
          </>
        )}
        <span className="grow" />
        {p.compatible && !pending && !(mode === "multi" && p.builtin) && (
          mode === "single" ? (
            <button className={`pbtn${isCurrent ? " on" : ""}`} aria-pressed={isCurrent} disabled={readonly || isCurrent} onClick={stop(onAction)}>
              {!isCurrent ? t("providerCard.setCurrent") : switching ? t("providerCard.switchOnApply") : t("providerCard.inUse")}
            </button>
          ) : (
            <button className={`pbtn${enabled ? " enabled" : ""}`} aria-pressed={enabled} disabled={readonly} onClick={stop(onAction)}>
              {enabled ? t("common.enabled") : t("common.disabled")}
            </button>
          )
        )}
      </div>
    </div>
  );
}
