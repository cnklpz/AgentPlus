import { useRef, useState } from "react";
import { type McpInput, type McpLink, type McpServer, type McpSource, type McpTransport, api } from "../api";
import { t, tn } from "../i18n";
import { errText } from "../util";
import { ErrorBox, Seg } from "./controls";
import { Modal } from "./Modal";
import { AgentIcon, Icon } from "./icons";

/** Protocol names: not translated. */
export const MCP_TRANSPORT: Record<McpTransport, string> = { stdio: "stdio", http: "HTTP", sse: "SSE", ws: "WebSocket", remote: "HTTP / SSE" };

/** A place a server can be written to: an agent, or the MCP library. */
export interface McpTarget {
  id: McpSource;
  name: string;
  /** Names of the servers it has (with pending changes). */
  names: Set<string>;
}

/** Why `target` can't take this server (null = it can). Mirrors the backend's checks. */
export function mcpBlocked(target: McpSource, transport: McpTransport, cwd: boolean): string | null {
  if (target === "library") return null;
  if (transport === "ws" && target !== "claude") return t("mcpDialog.noTransport", { transport: MCP_TRANSPORT.ws });
  if (transport === "sse" && (target === "codex" || target === "dsh")) return t("mcpDialog.noTransport", { transport: MCP_TRANSPORT.sse });
  if (transport === "stdio" && cwd && ["claude", "codebuddy", "droid", "hermes"].includes(target)) return t("mcpDialog.noCwd");
  return null;
}

/** Editing: the definition, where its values are read from, and who has it now. */
export interface McpEdit {
  s: McpServer;
  /** None: a pending server typed in by hand, with nothing masked. */
  from: [McpSource, string] | undefined;
  holders: McpSource[];
}

interface Props {
  edit: McpEdit | null;
  targets: McpTarget[];
  /** Adding from an import link: its servers fill the form, its agents are ticked. */
  link?: McpLink;
  /** Adding: ticked to begin with (the agent whose MCP tab it was opened from). */
  defaultTo?: McpSource[];
  /** False: the user backed out (the dialog stays open). */
  onSave: (input: McpInput, to: McpSource[], removeFrom: McpSource[]) => Promise<boolean>;
  onClose: () => void;
}

const lines = (text: string) => text.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);

/** `KEY=value` (env) or `Name: value` (headers), one per line. */
function pairs(text: string, sep: string): { key: string; value: string }[] {
  return lines(text).map((l) => {
    const at = l.indexOf(sep);
    return at < 0 ? { key: l, value: "" } : { key: l.slice(0, at).trim(), value: l.slice(at + sep.length).trim() };
  }).filter((p) => p.key);
}

const joinPairs = (kv: { key: string; value: string }[], sep: string) => kv.map((p) => `${p.key}${sep}${p.value}`).join("\n");

export function McpDialog({ edit, targets, link, defaultTo, onSave, onClose }: Props) {
  const s: McpServer | McpInput | undefined = edit?.s ?? link?.servers[0];
  const [name, setName] = useState(s?.name ?? "");
  const [transport, setTransport] = useState<McpTransport>(s?.transport ?? "stdio");
  const [command, setCommand] = useState(s?.command ?? "");
  const [args, setArgs] = useState(s?.args.join("\n") ?? "");
  const [cwd, setCwd] = useState(s?.cwd ?? "");
  const [url, setUrl] = useState(s?.url ?? "");
  const [env, setEnv] = useState(s ? joinPairs(s.env, "=") : "");
  const [headers, setHeaders] = useState(s ? joinPairs(s.headers, ": ") : "");
  const [extra, setExtra] = useState<Pick<McpInput, "extra" | "extraFamily">>(link ? { extra: link.servers[0].extra, extraFamily: link.servers[0].extraFamily } : {});
  const [to, setTo] = useState<Set<McpSource>>(new Set(edit?.holders ?? (link?.agents ?? defaultTo ?? []).filter((a) => targets.some((x) => x.id === a))));
  const [paste, setPaste] = useState<string | null>(null);
  const [parsed, setParsed] = useState<McpInput[]>(link?.servers ?? []);
  const [err, setErr] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const first = useRef<HTMLInputElement>(null);

  const stdio = transport === "stdio";
  const trimmed = name.trim();
  const fill = (i: McpInput) => {
    setName(i.name || name);
    setTransport(i.transport);
    setCommand(i.command ?? "");
    setArgs(i.args.join("\n"));
    setCwd(i.cwd ?? "");
    setUrl(i.url ?? "");
    setEnv(joinPairs(i.env, "="));
    setHeaders(joinPairs(i.headers, ": "));
    setExtra({ extra: i.extra, extraFamily: i.extraFamily });
  };
  const readPaste = async () => {
    setErr(null);
    try {
      const list = await api.mcpParse(paste ?? "");
      setParsed(list);
      fill(list[0]);
      if (list.length === 1) setPaste(null);
    } catch (e) {
      setErr(errText(e));
    }
  };

  const missing = !trimmed ? t("mcpDialog.needName") : stdio && !command.trim() ? t("mcpDialog.needCommand") : !stdio && !url.trim() ? t("mcpDialog.needUrl") : null;
  const blocked = (id: McpSource) => mcpBlocked(id, transport, !!cwd.trim());
  const chosen = [...to].filter((id) => !blocked(id));
  const removed = (edit?.holders ?? []).filter((id) => !to.has(id));
  const canSave = !missing && !saving && (chosen.length > 0 || removed.length > 0);

  const save = async () => {
    setSaving(true);
    setErr(null);
    try {
      const input: McpInput = {
        name: trimmed,
        transport,
        command: stdio ? command.trim() : null,
        args: stdio ? lines(args) : [],
        cwd: stdio && cwd.trim() ? cwd.trim() : null,
        url: stdio ? null : url.trim(),
        env: stdio ? pairs(env, "=") : [],
        headers: stdio ? [] : pairs(headers, ":"),
        enabled: true,
        ...(edit ? { ...(edit.from ? { from: edit.from } : {}), ...(edit.s.name !== trimmed ? { replaces: edit.s.name } : {}) } : extra),
      };
      if (await onSave(input, chosen, removed)) onClose();
      else setSaving(false);
    } catch (e) {
      setErr(errText(e));
      setSaving(false);
    }
  };

  const transports: McpTransport[] = ["stdio", "http", "sse", ...(transport === "remote" ? ["remote" as const] : []), "ws"];
  const foot = (
    <>
      <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={!canSave} title={missing ?? undefined} onClick={save}>{saving ? t("common.saving") : edit ? t("common.save") : t("common.add")}</button>
    </>
  );
  return (
    <Modal label={edit ? t("mcpDialog.editTitle", { name: edit.s.name }) : t("mcpDialog.addTitle")} title={edit ? t("mcpDialog.editTitle", { name: edit.s.name }) : t("mcpDialog.addTitle")}
      wide onClose={onClose} busy={saving} foot={foot}>
      {link && <span className="tiny muted">{tn("mcpDialog.fromLink", link.servers.length)}</span>}
      {link && parsed.length > 1 && paste === null && (
        <div className="row gap6 mcp-parsed">
          <span className="tiny muted">{t("mcpDialog.pickParsed")}</span>
          {parsed.map((p) => (
            <button key={p.name} type="button" className={`btn small${p.name === trimmed ? " primary" : ""}`} onClick={() => fill(p)}>{p.name}</button>
          ))}
        </div>
      )}
      {!edit && !link && (paste === null ? (
        <button type="button" className="btn small mcp-paste-btn" onClick={() => setPaste("")}><Icon.copy size={12} />{t("mcpDialog.pasteConfig")}</button>
      ) : (
        <div className="field">
          <span>{t("mcpDialog.pasteConfig")} <em className="muted tiny">{t("mcpDialog.pasteHint")}</em></span>
          <textarea className="input mono mcp-text" rows={6} value={paste} autoFocus onChange={(e) => setPaste(e.target.value)} placeholder={'{ "mcpServers": { … } }'} />
          <div className="row gap6">
            <button type="button" className="btn small primary" disabled={!paste.trim()} onClick={readPaste}>{t("mcpDialog.readPaste")}</button>
            <button type="button" className="btn small" onClick={() => { setPaste(null); setParsed([]); }}>{t("common.cancel")}</button>
          </div>
          {parsed.length > 1 && (
            <div className="row gap6 mcp-parsed">
              <span className="tiny muted">{t("mcpDialog.pickParsed")}</span>
              {parsed.map((p) => (
                <button key={p.name} type="button" className={`btn small${p.name === trimmed ? " primary" : ""}`} onClick={() => fill(p)}>{p.name || t("mcpDialog.unnamed")}</button>
              ))}
            </div>
          )}
        </div>
      ))}

      <div className="form2">
        <label className="field">
          <span>{t("common.name")}</span>
          <input ref={first} className="input mono" value={name} autoFocus={!!edit} onChange={(e) => setName(e.target.value)} placeholder="github" />
        </label>
        <div className="field">
          <span>{t("mcpDialog.transport")}</span>
          <Seg value={transport} onChange={setTransport} label={t("mcpDialog.transport")} options={transports.map((v) => ({ value: v, label: MCP_TRANSPORT[v] }))} />
        </div>
      </div>

      {stdio ? (
        <>
          <div className="form2">
            <label className="field">
              <span>{t("mcpDialog.command")}</span>
              <input className="input mono" value={command} onChange={(e) => setCommand(e.target.value)} placeholder="npx" />
            </label>
            <label className="field">
              <span>{t("mcpDialog.cwd")} <em className="muted tiny">{t("mcpDialog.optional")}</em></span>
              <input className="input mono" value={cwd} onChange={(e) => setCwd(e.target.value)} />
            </label>
          </div>
          <div className="form2">
            <label className="field">
              <span>{t("mcpDialog.args")} <em className="muted tiny">{t("mcpDialog.onePerLine")}</em></span>
              <textarea className="input mono mcp-text" rows={4} value={args} onChange={(e) => setArgs(e.target.value)} placeholder={"-y\n@modelcontextprotocol/server-filesystem"} />
            </label>
            <label className="field">
              <span>{t("mcpDialog.env")} <em className="muted tiny">KEY=value</em></span>
              <textarea className="input mono mcp-text sensitive" rows={4} value={env} onChange={(e) => setEnv(e.target.value)} placeholder={"GITHUB_TOKEN=${GITHUB_TOKEN}"} />
            </label>
          </div>
        </>
      ) : (
        <div className="form2">
          <label className="field">
            <span>{t("mcpDialog.url")}</span>
            <input className="input mono sensitive" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://example.com/mcp" />
          </label>
          <label className="field">
            <span>{t("mcpDialog.headers")} <em className="muted tiny">Name: value</em></span>
            <textarea className="input mono mcp-text sensitive" rows={3} value={headers} onChange={(e) => setHeaders(e.target.value)} placeholder={"Authorization: Bearer ${API_TOKEN}"} />
          </label>
        </div>
      )}
      <em className="muted tiny hint">{t("mcpDialog.varsHint")}</em>

      <div className="field">
        <span>{t("mcpDialog.writeTo")}</span>
        <div className="agent-picks">
          {targets.map((a) => {
            const why = blocked(a.id);
            const had = edit?.holders.includes(a.id) ?? false;
            const clash = !had && a.names.has(trimmed);
            return (
              <label key={a.id} className={`apick${to.has(a.id) && !why ? " on" : ""}${why ? " dim" : ""}`} title={why ?? undefined}>
                <input type="checkbox" disabled={!!why} checked={to.has(a.id) && !why}
                  onChange={() => setTo((p) => { const n = new Set(p); if (n.has(a.id)) n.delete(a.id); else n.add(a.id); return n; })} />
                {a.id === "library" ? <Icon.layers size={16} /> : <AgentIcon id={a.id} size={18} />}
                <span className="small">{a.name}</span>
                {why && <span className="tiny muted">{why}</span>}
                {!why && clash && to.has(a.id) && <span className="tiny warn-text">{t("mcpDialog.replaces")}</span>}
                {had && !to.has(a.id) && <span className="tiny warn-text">{t("mcpDialog.removes")}</span>}
              </label>
            );
          })}
        </div>
        <em className="muted tiny hint">{t("mcpDialog.writeHint")}</em>
      </div>

      {err && <ErrorBox text={err} />}
    </Modal>
  );
}
