import codexPng from "../assets/codex.png";
import zcodePng from "../assets/zcode.png";
import mimoPng from "../assets/mimo.png";
import type { AgentId } from "../api";

// Codex's app icon is a dark glyph on a light tile (bordered so it reads on the
// light sidebar); ZCode's icon has built-in padding (scaled up to fill); MiMo's
// fills its tile already.
const AGENT_ICONS: Record<AgentId, { src: string; bg: string; scale: number; ring?: string }> = {
  codex: { src: codexPng, bg: "#FFFFFF", scale: 66, ring: "inset 0 0 0 1px #D5DAE1" },
  zcode: { src: zcodePng, bg: "#000000", scale: 122 },
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
  grip: ({ size = 14 }: P) => (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="#B8BFC9" aria-hidden="true">
      {[6, 12, 18].map((y) => [9, 15].map((x) => <circle key={`${x}-${y}`} cx={x} cy={y} r="1.6" />))}
    </svg>
  ),
  logo: ({ size = 22 }: P) => (
    <svg width={size} height={size} viewBox="0 0 22 22" aria-hidden="true">
      <rect x="1" y="1" width="20" height="20" rx="6" fill="var(--accent)" />
      <path d="M11 6.5v9M6.5 11h9" stroke="#fff" strokeWidth="2.2" strokeLinecap="round" />
    </svg>
  ),
};
