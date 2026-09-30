import { api } from "../api";
import { type Template, VENDORS, planLabel } from "../templates";
import { Dropdown } from "./Dropdown";
import { Seg } from "./controls";
import { Icon, VendorIcon } from "./icons";
import { t } from "../i18n";

/** Dropdown value of the "from provider library" choice. */
const LIBRARY = "@library";

/** Vendor dropdown, plus a plan switch for vendors with both a coding plan and pay as you go.
 * With `library`, it also offers picking entries from the provider library. */
export function TemplatePicker({ value, onPick, library, vendors = true }: {
  value: Template | null;
  onPick: (tpl: Template | null) => void;
  library?: { on: boolean; onChange: (on: boolean) => void };
  /** Agents without a supported vendor protocol can still pick from the library. */
  vendors?: boolean;
}) {
  const available = vendors ? VENDORS : [];
  const vendor = value && !library?.on ? available.find((v) => v.id === value.icon) ?? null : null;
  return (
    <div className="field tpl-dd">
      <span className="field-label">{t("templatePicker.label")}</span>
      <div className="tpl-row">
        <Dropdown label={t("templatePicker.vendor")} value={library?.on ? LIBRARY : vendor?.id ?? ""} maxHeight={420}
          onChange={(v) => {
            library?.onChange(v === LIBRARY);
            if (v !== LIBRARY) onPick(available.find((x) => x.id === v)?.plans[0] ?? null);
          }}
          options={[
            { value: "", label: t("templatePicker.custom"), hint: t("templatePicker.customHint"), icon: <span className="tpl-none"><Icon.edit size={12} /></span> },
            ...(library ? [{ value: LIBRARY, label: t("templatePicker.library"), hint: t("templatePicker.libraryHint"), icon: <span className="tpl-none"><Icon.layers size={12} /></span> }] : []),
            ...available.map((v) => ({ value: v.id, label: v.name, hint: v.plans.map(planLabel).join(" / "), icon: <VendorIcon id={v.id} size={20} />, group: v.group })),
          ]} />
        {vendor && vendor.plans.length > 1 && (
          <Seg value={value?.id ?? ""} label={t("templatePicker.billing")} onChange={(id) => onPick(vendor.plans.find((p) => p.id === id) ?? null)}
            options={vendor.plans.map((p) => ({ value: p.id, label: planLabel(p) }))} />
        )}
      </div>
      {value?.note && !library?.on && <em className="muted tiny">{value.note}</em>}
    </div>
  );
}

/** The template vendor's page for getting a key (after the key field's hint, space-separated). */
export function TemplateKeyLink({ tpl }: { tpl: Template }) {
  return (
    <> <button type="button" className="link" onClick={() => api.openUrl(tpl.keyUrl).catch(() => undefined)}>{t("templatePicker.getKey", { vendor: tpl.vendor })}</button></>
  );
}
