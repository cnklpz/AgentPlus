import { api } from "../api";
import { type Template, VENDORS, planLabel } from "../templates";
import { Dropdown } from "./Dropdown";
import { Seg } from "./controls";
import { Icon, VendorIcon } from "./icons";
import { t } from "../i18n";

/** Vendor dropdown, plus a plan switch for vendors with both a coding plan and pay as you go. */
export function TemplatePicker({ value, onPick }: { value: Template | null; onPick: (tpl: Template | null) => void }) {
  const vendor = value ? VENDORS.find((v) => v.id === value.icon) ?? null : null;
  return (
    <div className="field tpl-dd">
      <span className="field-label">{t("templatePicker.label")} <em className="muted tiny hint">{t("templatePicker.labelHint")}</em></span>
      <div className="tpl-row">
        <Dropdown label={t("templatePicker.vendor")} value={vendor?.id ?? ""} maxHeight={420}
          onChange={(v) => onPick(VENDORS.find((x) => x.id === v)?.plans[0] ?? null)}
          options={[
            { value: "", label: t("templatePicker.custom"), hint: t("templatePicker.customHint"), icon: <span className="tpl-none"><Icon.edit size={12} /></span> },
            ...VENDORS.map((v) => ({ value: v.id, label: v.name, hint: v.plans.map(planLabel).join(" / "), icon: <VendorIcon id={v.id} size={20} />, group: v.group })),
          ]} />
        {vendor && vendor.plans.length > 1 && (
          <Seg value={value?.id ?? ""} label={t("templatePicker.billing")} onChange={(id) => onPick(vendor.plans.find((p) => p.id === id) ?? null)}
            options={vendor.plans.map((p) => ({ value: p.id, label: planLabel(p) }))} />
        )}
      </div>
      {value?.note && <em className="muted tiny">{value.note}</em>}
    </div>
  );
}

/** The template vendor's page for getting a key (after the key field's hint, space-separated). */
export function TemplateKeyLink({ tpl }: { tpl: Template }) {
  return (
    <> <button type="button" className="link" onClick={() => api.openUrl(tpl.keyUrl).catch(() => undefined)}>{t("templatePicker.getKey", { vendor: tpl.vendor })}</button></>
  );
}
