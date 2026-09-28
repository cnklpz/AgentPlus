import { useEffect, useMemo, useState } from "react";
import { type GatewayStatus, api } from "../api";
import { ComboBox } from "./ComboBox";
import { Switch } from "./controls";
import { locale, t, useLang } from "../i18n";
import { errText, type Flash } from "../util";

let zoneList: string[] | null = null;

/** IANA time zones this system knows, for the suggestion menu. */
function timeZones(): string[] {
  if (!zoneList) {
    const all = (Intl as unknown as { supportedValuesOf?: (key: string) => string[] }).supportedValuesOf?.("timeZone");
    zoneList = all?.length ? all : ["UTC", "Asia/Shanghai", "Asia/Singapore", "Asia/Tokyo", "Europe/London", "America/New_York", "America/Los_Angeles"];
  }
  return zoneList;
}

/** `zone` spelled as the system spells it ("asia/tokyo" → "Asia/Tokyo"), or null when it doesn't know it. */
function knownZone(zone: string): string | null {
  try {
    const canon = new Intl.DateTimeFormat("en-US", { timeZone: zone }).resolvedOptions().timeZone;
    // Only fix the case: the system may name a zone by an older alias.
    return canon.toLowerCase() === zone.toLowerCase() ? canon : zone;
  } catch {
    return null;
  }
}

/** A zone's name in the UI language plus its UTC offset: "中国标准时间 · GMT+8". */
function zoneName(zone: string, loc: string): string {
  const part = (style: string) => {
    try {
      const f = new Intl.DateTimeFormat(loc, { timeZone: zone, timeZoneName: style as Intl.DateTimeFormatOptions["timeZoneName"] });
      return f.formatToParts(new Date()).find((p) => p.type === "timeZoneName")?.value ?? "";
    } catch {
      return "";
    }
  };
  const offset = part("shortOffset");
  // The generic name has no daylight-saving variant; Chinese names that still carry an
  // English city ("Urumqi时间") or are only an offset are skipped.
  const name = [part("longGeneric"), part("long")].find((n) => n && !n.startsWith("GMT") && !(loc.startsWith("zh") && /[A-Za-z]/.test(n)));
  return name ? `${name} · ${offset}` : offset;
}

/**
 * Codex settings tab: the time zone the local gateway reports to the model in place of the
 * one Codex puts in its environment context. Saved right away (it's AgentPlus's own setting,
 * not part of Codex's config).
 */
export function CodexTimezone({ zone, setStatus, flash }: { zone: string | null; setStatus: (s: GatewayStatus) => void; flash: Flash }) {
  const lang = useLang();
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(zone ?? "");
  useEffect(() => { setText(zone ?? ""); }, [zone]);
  const loc = locale();
  const names = useMemo(() => new Map<string, string>(), [lang]);
  const nameOf = (z: string) => {
    let n = names.get(z);
    if (n === undefined) names.set(z, (n = zoneName(z, loc)));
    return n;
  };
  const on = zone !== null || editing;
  const name = text.trim();
  const known = name ? knownZone(name) : null;
  const dirty = name !== (zone ?? "");
  const system = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  const save = async (next: string | null) => {
    try {
      setStatus(await api.gatewaySetTimezone(next));
      setEditing(false);
      flash(next ? t("codexTimezone.saved", { zone: next }) : t("codexTimezone.off"));
    } catch (e) {
      flash(errText(e), true);
    }
  };
  const submit = () => { if (known && dirty) save(known); };
  return (
    <section className="sgroup">
      <h2>{t("codexTimezone.title")}</h2>
      <div className="srow">
        <div className="grow minw0">
          <div className="slabel">{t("codexTimezone.toggle")}</div>
          <div className="muted small hint">{t("codexTimezone.desc", { zone: t("codexTimezone.zoneWithName", { id: system, name: nameOf(system) }) })}</div>
        </div>
        <Switch on={on} label={t("codexTimezone.toggle")} onChange={() => (zone !== null ? save(null) : setEditing(!editing))} />
      </div>
      {on && (
        <div className="srow stacked">
          <div className="gw-test gw-tz-form">
            <ComboBox value={text} options={timeZones()} describe={nameOf} onChange={setText} onEnter={submit}
              placeholder={t("codexTimezone.placeholder")} label={t("codexTimezone.label")} />
            {dirty && known && <button className="btn small primary" onClick={submit}>{t("common.save")}</button>}
          </div>
          {name && (known
            ? <span className="tiny muted">{t("codexTimezone.now", { name: nameOf(known), time: new Date().toLocaleString(loc, { timeZone: known, dateStyle: "medium", timeStyle: "short" }) })}</span>
            : <em className="field-err">{t("codexTimezone.invalid")}</em>)}
        </div>
      )}
    </section>
  );
}
