import codexPng from "../assets/codex.png";
import zcodePng from "../assets/zcode.png";
import mimoPng from "../assets/mimo.png";
// Marks from lobehub/lobe-icons (MIT), except pi (pi.dev), OpenCode (opencode.ai's
// app icon) and Droid (factory.ai's favicon).
import claudeSvg from "../assets/claude.svg";
import geminiSvg from "../assets/gemini.svg";
import geminiMarkSvg from "../assets/gemini-mark.svg";
import qwenSvg from "../assets/qwen.svg";
import kimiSvg from "../assets/kimi.svg";
import codebuddySvg from "../assets/codebuddy.svg";
import kiloSvg from "../assets/kilo.svg";
import traeSvg from "../assets/trae.svg";
import hermesSvg from "../assets/hermes.svg";
import openclawSvg from "../assets/openclaw.svg";
import piSvg from "../assets/pi.svg";
import opencodeSvg from "../assets/opencode.svg";
import droidSvg from "../assets/droid.svg";
import type { AgentId } from "../api";

/** A letter tile, for vendors without an icon of their own. */
function mono(text: string, bg: string, fg: string, size = 14) {
  return {
    bg, scale: 100,
    glyph: (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <text x="12" y="12" dominantBaseline="central" textAnchor="middle" fontSize={size} fontWeight={800} fill={fg} fontFamily="Segoe UI, sans-serif">{text}</text>
      </svg>
    ),
  };
}

// Codex's app icon is a dark glyph on a light tile (outlined so it reads on the
// light sidebar); ZCode's, MiMo's, Gemini CLI's and CodeBuddy's images are their own tiles.
type Tile = { src?: string; glyph?: React.ReactNode; bg: string; scale: number; ring?: string };
const LIGHT_RING = "inset 0 0 0 1px #D5DAE1";
const DARK_RING = "inset 0 0 0 1px #2A2A2E";
const AGENT_ICONS: Record<AgentId, Tile> = {
  codex: { src: codexPng, bg: "#FFFFFF", scale: 66, ring: LIGHT_RING },
  zcode: { src: zcodePng, bg: "#000000", scale: 100 },
  mimo: { src: mimoPng, bg: "#000000", scale: 100 },
  claude: { src: claudeSvg, bg: "#D97757", scale: 64 },
  hermes: { src: hermesSvg, bg: "#0B0B0C", scale: 78, ring: DARK_RING },
  gemini: { src: geminiSvg, bg: "#1E1E2E", scale: 100 },
  pi: { src: piSvg, bg: "#111111", scale: 52, ring: DARK_RING },
  openclaw: { src: openclawSvg, bg: "#050810", scale: 78 },
  qwen: { src: qwenSvg, bg: "#FFFFFF", scale: 70, ring: LIGHT_RING },
  kimi: { src: kimiSvg, bg: "#000000", scale: 70 },
  droid: { src: droidSvg, bg: "#020202", scale: 96 },
  codebuddy: { src: codebuddySvg, bg: "#6C4DFF", scale: 100 },
  kilo: { src: kiloSvg, bg: "#F8F675", scale: 66 },
  trae: { src: traeSvg, bg: "#0B0B0C", scale: 72 },
  opencode: { src: opencodeSvg, bg: "#131010", scale: 78, ring: DARK_RING },
};

export function AgentIcon({ id, size }: { id: AgentId; size: number }) {
  // Project configs ("opencode@<folder>") use their agent's icon.
  return <TileIcon i={AGENT_ICONS[id.split("@")[0] as AgentId] ?? AGENT_ICONS.codex} size={size} />;
}

/** Model vendors, for provider templates. */
const VENDOR_ICONS: Record<string, Tile> = {
  mimo: AGENT_ICONS.mimo,
  volcengine: mono("火", "#1664FF", "#FFFFFF", 13),
  zhipu: mono("智", "#2D5BFF", "#FFFFFF", 13),
  kimi: AGENT_ICONS.kimi,
  deepseek: mono("DS", "#4D6BFE", "#FFFFFF", 10),
  bailian: mono("百", "#FF6A00", "#FFFFFF", 13),
  minimax: mono("M", "#E8374F", "#FFFFFF"),
  siliconflow: mono("硅", "#6E44FF", "#FFFFFF", 13),
  openrouter: mono("OR", "#111827", "#FFFFFF", 10),
  opencode: AGENT_ICONS.opencode,
  tencent: mono("腾", "#0052D9", "#FFFFFF", 13),
  qianfan: mono("千", "#2932E1", "#FFFFFF", 13),
  openai: AGENT_ICONS.codex,
  anthropic: AGENT_ICONS.claude,
  // Gemini the model family: the sparkle, not Gemini CLI's app tile.
  gemini: { src: geminiMarkSvg, bg: "#FFFFFF", scale: 70, ring: LIGHT_RING },
};

export function VendorIcon({ id, size }: { id: string; size: number }) {
  return <TileIcon i={VENDOR_ICONS[id] ?? mono(id.slice(0, 1).toUpperCase(), "#64748B", "#FFFFFF")} size={size} />;
}

function TileIcon({ i, size }: { i: Tile; size: number }) {
  const box = { width: `${i.scale}%`, height: `${i.scale}%` };
  return (
    <span className="agent-icon" style={{ width: size, height: size, borderRadius: Math.round(size * 0.24), background: i.bg, boxShadow: i.ring }}>
      {i.src ? <img src={i.src} alt="" style={box} /> : <span style={{ ...box, display: "grid" }}>{i.glyph}</span>}
    </span>
  );
}

/** `sw`: stroke width, for the few places that draw an icon heavier or lighter. */
type P = { size?: number; color?: string; sw?: number };
const svg = (size: number, color: string, children: React.ReactNode, sw: number) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={sw} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {children}
  </svg>
);

export const Icon = {
  search: ({ size = 15, color = "currentColor", sw = 2 }: P) => svg(size, color, <><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></>, sw),
  monitor: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <><rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 20h8M12 16v4" /></>, sw),
  folder: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" />, sw),
  refresh: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <><path d="M21 12a9 9 0 1 1-2.6-6.4L21 8" /><path d="M21 3v5h-5" /></>, sw),
  download: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <><path d="M12 4v11" /><path d="m7 10 5 5 5-5" /><path d="M5 20h14" /></>, sw),
  external: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <><path d="M14 4h6v6" /><path d="M20 4 11 13" /><path d="M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4" /></>, sw),
  play: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <path d="M7 4.5v15l12.5-7.5Z" />, sw),
  layers: ({ size = 16, color = "currentColor", sw = 2 }: P) => svg(size, color, <><path d="m12 3 9 5-9 5-9-5 9-5Z" /><path d="m3 13 9 5 9-5" /></>, sw),
  history: ({ size = 16, color = "currentColor", sw = 2 }: P) => svg(size, color, <><path d="M3 12a9 9 0 1 0 3-6.7L3 8" /><path d="M3 3v5h5M12 7v5l3 2" /></>, sw),
  cloud: ({ size = 16, color = "currentColor", sw = 2 }: P) => svg(size, color, <path d="M17.5 19a4.5 4.5 0 1 0-1.4-8.8A6 6 0 0 0 4.5 13 3.5 3.5 0 0 0 6 19Z" />, sw),
  plus: ({ size = 14, color = "currentColor", sw = 2.4 }: P) => svg(size, color, <path d="M12 5v14M5 12h14" />, sw),
  check: ({ size = 14, color = "currentColor", sw = 2.4 }: P) => svg(size, color, <path d="M20 6 9 17l-5-5" />, sw),
  pulse: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <path d="M22 12h-4l-3 9L9 3l-3 9H2" />, sw),
  gear: ({ size = 16, color = "currentColor", sw = 1.8 }: P) => svg(size, color, <><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z" /></>, sw),
  chevron: ({ size = 12, color = "currentColor", sw = 2.2 }: P) => svg(size, color, <path d="m6 9 6 6 6-6" />, sw),
  close: ({ size = 12, color = "currentColor", sw = 2.2 }: P) => svg(size, color, <path d="M6 6l12 12M18 6 6 18" />, sw),
  edit: ({ size = 13, color = "currentColor", sw = 2 }: P) => svg(size, color, <><path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" /></>, sw),
  trash: ({ size = 13, color = "currentColor", sw = 2 }: P) => svg(size, color, <><path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6" /></>, sw),
  copy: ({ size = 13, color = "currentColor", sw = 2 }: P) => svg(size, color, <><rect x="9" y="9" width="12" height="12" rx="2" /><path d="M5 15V5a2 2 0 0 1 2-2h10" /></>, sw),
  key: ({ size = 13, color = "currentColor", sw = 2 }: P) => svg(size, color, <><circle cx="7.5" cy="15.5" r="4.5" /><path d="m10.7 12.3 9.8-9.8M17 6l3 3M15 8l2 2" /></>, sw),
  gateway: ({ size = 16, color = "currentColor", sw = 2 }: P) => svg(size, color, <><rect x="3" y="4" width="18" height="6" rx="2" /><rect x="3" y="14" width="18" height="6" rx="2" /><path d="M7 7h.01M7 17h.01M11 7h6M11 17h6" /></>, sw),
  back: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <path d="M19 12H5M11 18l-6-6 6-6" />, sw),
  arrow: ({ size = 13, color = "currentColor", sw = 2 }: P) => svg(size, color, <path d="M5 12h14M13 6l6 6-6 6" />, sw),
  sun: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></>, sw),
  moon: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <path d="M20.5 14.1A8.5 8.5 0 1 1 9.9 3.5a6.6 6.6 0 0 0 10.6 10.6Z" />, sw),
  power: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <><path d="M12 3v8" /><path d="M6.4 6.6a8 8 0 1 0 11.2 0" /></>, sw),
  terminal: ({ size = 14, color = "currentColor", sw = 2 }: P) => svg(size, color, <><rect x="3" y="4" width="18" height="16" rx="2" /><path d="m7 9 3 3-3 3M13 15h4" /></>, sw),
  warn: ({ size = 12, color = "currentColor", sw = 2.2 }: P) => svg(size, color, <><path d="M12 9v4M12 17h.01" /><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z" /></>, sw),
  grip: ({ size = 14 }: P) => (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="#B8BFC9" aria-hidden="true">
      {[6, 12, 18].map((y) => [9, 15].map((x) => <circle key={`${x}-${y}`} cx={x} cy={y} r="1.6" />))}
    </svg>
  ),
  logo: ({ size = 22 }: P) => (
    // Same drawing as the app icon (src-tauri/app-icon.svg), without the drop shadow.
    <svg width={size} height={size} viewBox="80 80 864 864" aria-hidden="true">
      <defs>
        <linearGradient id="ap-logo-bg" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#5B7BFF" />
          <stop offset="0.55" stopColor="#2F54EB" />
          <stop offset="1" stopColor="#3423C9" />
        </linearGradient>
      </defs>
      <rect x="80" y="80" width="864" height="864" rx="208" fill="url(#ap-logo-bg)" />
      <rect x="352" y="232" width="440" height="440" rx="104" fill="#FFFFFF" fillOpacity="0.34" />
      <rect x="232" y="352" width="440" height="440" rx="104" fill="#FFFFFF" />
      <path d="M452 480V664M360 572H544" stroke="#3448E6" strokeWidth="84" strokeLinecap="round" fill="none" />
    </svg>
  ),
};

/** Where configs are read and written: Windows (a monitor) or a WSL distro (a terminal). */
export function EnvIcon({ id, size }: { id: string; size?: number }) {
  return id.startsWith("wsl:") ? <Icon.terminal size={size} /> : <Icon.monitor size={size} />;
}

/** The square tick box of a multi-select option (`.opt-check`), ticked when `on`. */
export function OptCheck({ on }: { on: boolean }) {
  return <span className="opt-check" aria-hidden="true">{on && <Icon.check size={10} color="#fff" sw={3.5} />}</span>;
}
