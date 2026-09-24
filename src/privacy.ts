// 隐私模式 (privacy mode): API keys, provider addresses, the user name in folder paths and
// conversation text are masked or blurred, so the window can be shown in screenshots,
// screen shares and recordings. Only what is displayed changes; nothing is written.
import { createStore } from "./store";

let on = false;
const store = createStore(() => on);

export function setPrivacy(v: boolean): void {
  if (v === on) return;
  on = v;
  // CSS hook: `.sensitive` blocks (conversation text, inputs) blur while this is set.
  if (typeof document !== "undefined") document.documentElement.toggleAttribute("data-privacy", v);
  store.notify();
}

/** Re-render when privacy mode is switched. */
export function usePrivacy(): boolean {
  return store.use();
}

export const DOTS = "•••";

/** This machine (the gateway, local servers): nothing to hide. */
const LOCAL_HOST = /^(localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[::1\])$/i;

/** host[:port] → "•••" unless it is this machine. */
export function maskHost(host: string): string {
  const name = host.replace(/^[^@]*@/, "").replace(/:\d+$/, "");
  return LOCAL_HOST.test(name) ? host : DOTS;
}

const URL_RE = /\b((?:https?|wss?):\/\/)([^\s/?#"'<>()，。）]+)/gi;
const IPV4_RE = /\b(\d{1,3}(?:\.\d{1,3}){3})(:\d{1,5})?\b/g;
/** The folder after Users / home is the account name (Windows, macOS, Linux, WSL). */
const USER_DIR_RE = /((?:^|[\\/])(?:Users|home)[\\/])([^\\/\s"'<>]+)/gi;
/** Prefixed API keys (sk-…, sk-ant-…, AIza…) and "Bearer …". */
const KEY_RE = /\b(?:(?:sk|ak|pk|rk)-[A-Za-z0-9_-]{6,}|AIza[A-Za-z0-9_-]{20,})|(\bBearer\s+)[A-Za-z0-9._~+/=-]{8,}/g;
/** Long random-looking tokens (letters and digits mixed). */
const TOKEN_RE = /\b(?=[A-Za-z0-9_]*\d)(?=[A-Za-z0-9_]*[A-Za-z])[A-Za-z0-9_]{32,}\b/g;
/** Key hints like "••••abcd": the visible tail goes too. */
const HINT_RE = /[•*]{3,}[A-Za-z0-9_-]{1,8}\b/g;

/** Masks everything sensitive in `text` (URL hosts, IPs, user folders, keys), whatever the mode. */
export function scrubAlways(text: string): string {
  return text
    .replace(KEY_RE, (_m, bearer?: string) => (bearer ?? "") + DOTS)
    .replace(URL_RE, (_m, scheme: string, host: string) => scheme + maskHost(host))
    .replace(IPV4_RE, (m, ip: string) => (maskHost(ip) === ip ? m : DOTS))
    .replace(USER_DIR_RE, (_m, dir: string) => dir + DOTS)
    .replace(TOKEN_RE, DOTS)
    .replace(HINT_RE, "••••");
}

/** `scrubAlways` while privacy mode is on; otherwise the text unchanged. For any displayed
 *  text that may carry an address, key or path (URLs, paths, backend notes, errors, diffs). */
export function scrub(text: string): string;
export function scrub(text: string | null | undefined): string | null | undefined;
export function scrub(text: string | null | undefined): string | null | undefined {
  return on && text ? scrubAlways(text) : text;
}

/** A host with an optional path and no scheme ("relay.example.com/v1", "64.83.33.80:8080"),
 *  as provider cards show it: the host goes. Plain names ("OpenRouter") stay. */
export function scrubHost(s: string): string;
export function scrubHost(s: string | null | undefined): string | null | undefined;
export function scrubHost(s: string | null | undefined): string | null | undefined {
  if (!on || !s) return s;
  const cut = s.indexOf("/");
  const head = cut < 0 ? s : s.slice(0, cut);
  if (!/[.:]/.test(head)) return scrubAlways(s);
  return maskHost(head) + (cut < 0 ? "" : scrubAlways(s.slice(cut)));
}

/** A `{name}` value filled into translated text, masked by what the placeholder holds. */
export function scrubVar(name: string, v: string): string {
  if (!on) return v;
  if (name === "machine") return DOTS;
  if (name === "host" || name === "station") return scrubHost(v);
  return scrubAlways(v);
}
