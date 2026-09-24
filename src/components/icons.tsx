import codexPng from "../assets/codex.png";
import zcodePng from "../assets/zcode.png";
import mimoPng from "../assets/mimo.png";
import type { AgentId } from "../api";

/** A letter tile, for agents without an app icon of their own. */
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
// light sidebar); ZCode's and MiMo's images are cropped to their own tiles.
type Tile = { src?: string; glyph?: React.ReactNode; bg: string; scale: number; ring?: string };
const AGENT_ICONS: Record<AgentId, Tile> = {
  codex: { src: codexPng, bg: "#FFFFFF", scale: 66, ring: "inset 0 0 0 1px #D5DAE1" },
  zcode: { src: zcodePng, bg: "#000000", scale: 100 },
  mimo: { src: mimoPng, bg: "#000000", scale: 100 },
  // Claude's starburst on its coral tile.
  claude: {
    bg: "#D97757", scale: 64,
    glyph: (
      <svg viewBox="0 0 24 24" fill="#FFFFFF" aria-hidden="true">
        {[0, 30, 60, 90, 120, 150].map((a) => <rect key={a} x="11" y="2" width="2" height="20" rx="1" transform={`rotate(${a} 12 12)`} />)}
      </svg>
    ),
  },
  hermes: mono("H", "#1E1B4B", "#C4B5FD"),
  gemini: {
    bg: "#FFFFFF", scale: 70, ring: "inset 0 0 0 1px #D5DAE1",
    glyph: (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <defs><linearGradient id="gem-g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stopColor="#4285F4" /><stop offset="1" stopColor="#A142F4" /></linearGradient></defs>
        <path d="M12 2c.6 5.3 4.7 9.4 10 10-5.3.6-9.4 4.7-10 10-.6-5.3-4.7-9.4-10-10 5.3-.6 9.4-4.7 10-10Z" fill="url(#gem-g)" />
      </svg>
    ),
  },
  pi: mono("π", "#111827", "#FFFFFF", 17),
  openclaw: mono("OC", "#DC2626", "#FFFFFF", 10),
  qwen: mono("Q", "#615CED", "#FFFFFF"),
  kimi: mono("K", "#000000", "#FFFFFF"),
  droid: mono("D", "#F97316", "#FFFFFF"),
  codebuddy: mono("CB", "#2563EB", "#FFFFFF", 10),
  kilo: mono("K", "#F8F675", "#1F2937"),
  trae: mono("T", "#0B0B0C", "#32F08C"),
  // OpenCode: a terminal-style mark on a dark tile.
  opencode: {
    bg: "#0B0B0C", scale: 62, ring: "inset 0 0 0 1px #2A2A2E",
    glyph: (
      <svg viewBox="0 0 24 24" fill="none" stroke="#FFFFFF" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <rect x="3" y="4" width="18" height="16" rx="3" strokeOpacity=".45" />
        <path d="m8 10 3 2.5L8 15M13 15h4" />
      </svg>
    ),
  },
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
  tencent: mono("腾", "#0052D9", "#FFFFFF", 13),
  qianfan: mono("千", "#2932E1", "#FFFFFF", 13),
  openai: AGENT_ICONS.codex,
  anthropic: AGENT_ICONS.claude,
  gemini: AGENT_ICONS.gemini,
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

type P = { size?: number; color?: string };
const svg = (size: number, color: string, children: React.ReactNode, sw = 2) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={sw} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {children}
  </svg>
);

export const Icon = {
  search: ({ size = 15, color = "currentColor" }: P) => svg(size, color, <><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></>),
  monitor: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <><rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 20h8M12 16v4" /></>),
  folder: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" />),
  refresh: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <><path d="M21 12a9 9 0 1 1-2.6-6.4L21 8" /><path d="M21 3v5h-5" /></>),
  play: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <path d="M7 4.5v15l12.5-7.5Z" />),
  layers: ({ size = 16, color = "currentColor" }: P) => svg(size, color, <><path d="m12 3 9 5-9 5-9-5 9-5Z" /><path d="m3 13 9 5 9-5" /></>),
  history: ({ size = 16, color = "currentColor" }: P) => svg(size, color, <><path d="M3 12a9 9 0 1 0 3-6.7L3 8" /><path d="M3 3v5h5M12 7v5l3 2" /></>),
  cloud: ({ size = 16, color = "currentColor" }: P) => svg(size, color, <path d="M17.5 19a4.5 4.5 0 1 0-1.4-8.8A6 6 0 0 0 4.5 13 3.5 3.5 0 0 0 6 19Z" />),
  plus: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <path d="M12 5v14M5 12h14" />, 2.4),
  check: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <path d="M20 6 9 17l-5-5" />, 2.4),
  pulse: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <path d="M22 12h-4l-3 9L9 3l-3 9H2" />),
  gear: ({ size = 16, color = "currentColor" }: P) => svg(size, color, <><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z" /></>, 1.8),
  chevron: ({ size = 12, color = "currentColor" }: P) => svg(size, color, <path d="m6 9 6 6 6-6" />, 2.2),
  close: ({ size = 12, color = "currentColor" }: P) => svg(size, color, <path d="M6 6l12 12M18 6 6 18" />, 2.2),
  edit: ({ size = 13, color = "currentColor" }: P) => svg(size, color, <><path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" /></>),
  trash: ({ size = 13, color = "currentColor" }: P) => svg(size, color, <><path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6" /></>),
  copy: ({ size = 13, color = "currentColor" }: P) => svg(size, color, <><rect x="9" y="9" width="12" height="12" rx="2" /><path d="M5 15V5a2 2 0 0 1 2-2h10" /></>),
  key: ({ size = 13, color = "currentColor" }: P) => svg(size, color, <><circle cx="7.5" cy="15.5" r="4.5" /><path d="m10.7 12.3 9.8-9.8M17 6l3 3M15 8l2 2" /></>),
  gateway: ({ size = 16, color = "currentColor" }: P) => svg(size, color, <><rect x="3" y="4" width="18" height="6" rx="2" /><rect x="3" y="14" width="18" height="6" rx="2" /><path d="M7 7h.01M7 17h.01M11 7h6M11 17h6" /></>),
  back: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <path d="M19 12H5M11 18l-6-6 6-6" />),
  arrow: ({ size = 13, color = "currentColor" }: P) => svg(size, color, <path d="M5 12h14M13 6l6 6-6 6" />),
  sun: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></>),
  moon: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <path d="M20.5 14.1A8.5 8.5 0 1 1 9.9 3.5a6.6 6.6 0 0 0 10.6 10.6Z" />),
  terminal: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <><rect x="3" y="4" width="18" height="16" rx="2" /><path d="m7 9 3 3-3 3M13 15h4" /></>),
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
