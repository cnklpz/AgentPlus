import { useEffect, useRef, useState } from "react";
import { type ImportRequest, api } from "../api";
import { Icon } from "./icons";
import { Modal } from "./Modal";
import { ErrorBox } from "./controls";
import { t } from "../i18n";
import { scrubHost } from "../privacy";
import { errText } from "../util";

const host = (url: string) => {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
};

/** Top of an add-provider dialog filled in from an import link: where it came from. */
export function ImportNote({ req }: { req: ImportRequest }) {
  return (
    <div className="import-note">
      <Icon.link size={16} />
      <div className="grow minw0">
        <div className="small strong">
          {t(req.source === "ccswitch" ? "importLink.fromCcswitch" : "importLink.fromAgentplus", { host: scrubHost(host(req.homepage ?? req.baseUrl)) })}
        </div>
        <div className="tiny muted">{t("importLink.check")}</div>
      </div>
      {req.homepage?.startsWith("https://") && (
        <button type="button" className="btn small" onClick={() => api.openUrl(req.homepage!).catch(() => undefined)}>
          <Icon.external size={12} />{t("importLink.homepage")}
        </button>
      )}
    </div>
  );
}

/** Paste an import link (the "Import to CC Switch" kind) instead of clicking it. */
export function ImportLinkDialog({ onImport, onClose }: { onImport: (r: ImportRequest) => void; onClose: () => void }) {
  const [link, setLink] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const first = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { first.current?.focus(); }, []);

  const go = async () => {
    if (!link.trim() || busy) return;
    setBusy(true);
    setErr(null);
    try {
      onImport(await api.parseImportLink(link.trim()));
    } catch (e) {
      setErr(errText(e));
      setBusy(false);
    }
  };

  const foot = (
    <>
      <span className="grow" />
      <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={!link.trim() || busy} onClick={() => { void go(); }}>{t("importLink.next")}</button>
    </>
  );
  return (
    <Modal label={t("importLink.title")} title={t("importLink.title")} onClose={onClose} foot={foot}>
      <div className="field">
        <label htmlFor="il-link">{t("importLink.linkLabel")}</label>
        <textarea id="il-link" ref={first} className="input mono sensitive import-link-input" rows={4} spellCheck={false} value={link}
          placeholder="ccswitch://v1/import?resource=provider&app=claude&name=…"
          onChange={(e) => { setLink(e.target.value); setErr(null); }}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void go(); } }} />
        <em className="muted tiny hint">{t("importLink.linkHint")}</em>
      </div>
      {err && <ErrorBox text={err} />}
    </Modal>
  );
}
