import type { KeyboardEvent, MouseEvent } from "react";
import type { Provider } from "../api";
import type { ViewProvider } from "../draft";

export type Latency = number | "pending" | string | undefined;

const PALETTE = ["#0F766E", "#7C3AED", "#1F2937", "#C2410C", "#2F54EB", "#B45309", "#0E7490", "#9D174D"];

/** "host:port/path" → "host:port"; used to spot the same service across agents. */
export function serviceKey(p: Provider): string {
  return p.baseUrl ? p.host.split("/")[0] : "";
}

/** Same service host → same avatar color, so duplicates across agents read as one. */
export function colorFor(p: Provider): string {
  if (p.builtin) return "#121722";
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
  if (!p.compatible) text = p.reason ?? "不兼容";
  else if (off) text = "停用中，不测速";
  else if (!p.baseUrl) text = "账号登录";
  else if (latency === "pending") text = "测速中…";
  else if (typeof latency === "number") text = `${latency} ms`;
  else text = latency ?? "未测速";
  return { text, level, live };
}

export function Bars({ level }: { level: number }) {
  return (
    <span className="bars" aria-hidden="true">
      {[5, 8, 12].map((h, i) => (
        <span key={h} style={{ height: h, background: i < level ? (level >= 2 ? "#16A34A" : "#D97706") : "#D5DAE1" }} />
      ))}
    </span>
  );
}

interface Props {
  p: ViewProvider;
  mode: "single" | "multi";
  isCurrent: boolean;
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

export function ProviderCard({ p, mode, isCurrent, selected, enabled, visible, latency, readonly, onSelect, onAction, onModels, onTest }: Props) {
  const off = mode === "multi" && !enabled;
  let tag: { text: string; cls: string } | null = null;
  if (p.isDeleted) tag = { text: "将删除", cls: "tag-del" };
  else if (p.isNew) tag = { text: "新 · 未应用", cls: "tag-new" };
  else if (!p.compatible) tag = { text: "不兼容", cls: "tag-muted" };
  else if (off) tag = { text: "已停用", cls: "tag-muted" };
  else if (isCurrent) tag = { text: "当前", cls: "tag-accent" };
  else if (p.builtin) tag = { text: "内置", cls: "tag-soft" };

  const lat = latencyView(p, off, latency);
  const total = p.models.length;
  const stop = (fn: () => void) => (e: MouseEvent) => { e.stopPropagation(); fn(); };
  const onKey = (e: KeyboardEvent) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onSelect(); } };

  const pending = p.isNew || p.isDeleted;
  return (
    <div
      className={`pcard${isCurrent ? " current" : ""}${selected ? " selected" : ""}${off || !p.compatible || p.isDeleted ? " dim" : ""}${p.isNew ? " fresh" : ""}`}
      role="button"
      tabIndex={0}
      aria-pressed={selected}
      aria-label={`${p.name} 详情`}
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
          <span className="pcard-host mono ellipsis">{p.host || "—"}</span>
        </span>
      </div>
      <div className="pcard-meta">
        {p.apis.map((a) => <span key={a} className="api-chip">{a}</span>)}
        {p.isEdited && !p.isDeleted && <span className="api-chip edited">已修改</span>}
        {total > 0 && p.compatible && !pending && (
          <button className="link" onClick={stop(onModels)}>{visible}/{total} 个模型可见</button>
        )}
        {p.isNew && <span className="tiny muted">{total} 个模型</span>}
      </div>
      <div className="pcard-foot">
        {p.baseUrl && p.compatible && !off && !pending ? (
          <button className="lat-btn" title="点一下重新测速" onClick={stop(onTest)} disabled={latency === "pending"}>
            <Bars level={lat.level} />
            <span className={`lat${lat.live ? (lat.level >= 2 ? " good" : " slow") : ""}`}>{lat.text}</span>
            <span className="lat-re" aria-hidden="true">↻</span>
          </button>
        ) : (
          <>
            <Bars level={0} />
            <span className="lat">{pending ? (p.isNew ? "应用后生效" : "应用后删除") : lat.text}</span>
          </>
        )}
        <span className="grow" />
        {p.compatible && !pending && !(mode === "multi" && p.builtin) && (
          mode === "single" ? (
            <button className={`pbtn${isCurrent ? " on" : ""}`} aria-pressed={isCurrent} disabled={readonly || isCurrent} onClick={stop(onAction)}>
              {isCurrent ? "正在使用" : "设为当前"}
            </button>
          ) : (
            <button className={`pbtn${enabled ? " enabled" : ""}`} aria-pressed={enabled} disabled={readonly} onClick={stop(onAction)}>
              {enabled ? "已启用" : "已停用"}
            </button>
          )
        )}
      </div>
    </div>
  );
}
