import codexPng from "../assets/codex.png";
import zcodePng from "../assets/zcode.png";
import mimoPng from "../assets/mimo.png";
import type { AgentId } from "../api";

// Codex's app icon is a dark glyph on a light tile (outlined so it reads on the
// light sidebar); ZCode's and MiMo's images are cropped to their own tiles.
const AGENT_ICONS: Record<AgentId, { src: string; bg: string; scale: number; ring?: string }> = {
  codex: { src: codexPng, bg: "#FFFFFF", scale: 66, ring: "inset 0 0 0 1px #D5DAE1" },
  zcode: { src: zcodePng, bg: "#000000", scale: 100 },
  mimo: { src: mimoPng, bg: "#000000", scale: 100 },
};

export function AgentIcon({ id, size }: { id: AgentId; size: number }) {
  const i = AGENT_ICONS[id];
  return (
    <span className="agent-icon" style={{ width: size, height: size, borderRadius: Math.round(size * 0.24), background: i.bg, boxShadow: i.ring }}>
      <img src={i.src} alt="" style={{ width: `${i.scale}%`, height: `${i.scale}%` }} />
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
  back: ({ size = 14, color = "currentColor" }: P) => svg(size, color, <path d="M19 12H5M11 18l-6-6 6-6" />),
  arrow: ({ size = 13, color = "currentColor" }: P) => svg(size, color, <path d="M5 12h14M13 6l6 6-6 6" />),
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
