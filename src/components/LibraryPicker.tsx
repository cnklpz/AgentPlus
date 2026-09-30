import { useState } from "react";
import type { AgentId } from "../api";
import { t } from "../i18n";
import { API_LABEL, type Group, USE_LABEL, cannotAdd, hostKey } from "../services";
import { Avatar, avatarColor } from "./ProviderCard";
import { scrubHost } from "../privacy";

/** Why a library entry can't be picked for `agent`: the agent has it (or has a pending
 * change to it, undone on the Providers page), or can't take it. Null when it can be added. */
export function libraryBlock(g: Group, agent: AgentId): string | null {
  const use = g.uses.find((u) => u.agent.id === agent);
  return use ? USE_LABEL[use.state] : cannotAdd(g, agent);
}

/** Provider library entries for the add-provider dialog, two per row; several can be picked
 * and are added together (address and key come from the library, in the backend). */
export function LibraryPicker({ agent, groups, picked, onToggle }: {
  agent: AgentId;
  /** The library's groups (hub groups with a library entry). */
  groups: Group[];
  picked: Set<string>;
  onToggle: (key: string) => void;
}) {
  const [q, setQ] = useState("");
  if (!groups.length) return <div className="empty">{t("libraryPicker.empty")}</div>;
  const needle = q.trim().toLowerCase();
  const shown = groups.filter((g) => !needle || `${g.name} ${g.baseUrl}`.toLowerCase().includes(needle));
  return (
    <div className="field">
      {groups.length > 6 && (
        <input className="search-input" autoFocus placeholder={t("libraryPicker.filterPlaceholder")} value={q} onChange={(e) => setQ(e.target.value)} />
      )}
      <div className="lib-grid">
        {shown.map((g) => {
          const why = libraryBlock(g, agent);
          const host = scrubHost(g.baseUrl.replace(/^https?:\/\//, ""));
          const on = picked.has(g.key);
          return (
            <label key={g.key} className={`lib-card${on ? " on" : ""}${why ? " dim" : ""}`} title={why ?? undefined}>
              <input type="checkbox" disabled={!!why} checked={on} onChange={() => onToggle(g.key)} />
              <Avatar small name={g.name} color={avatarColor(hostKey(g.baseUrl) || g.name)} />
              <span className="grow minw0">
                <span className="block small strong ellipsis" title={g.name}>{g.name}</span>
                <span className={`block tiny muted ellipsis${why ? "" : " mono"}`} title={why ?? host}>{why ?? host}</span>
              </span>
              <span className={`api-chip api-${g.api}`}>{API_LABEL[g.api]}</span>
            </label>
          );
        })}
      </div>
      {shown.length === 0 && <div className="empty">{t("libraryPicker.noMatch")}</div>}
    </div>
  );
}
