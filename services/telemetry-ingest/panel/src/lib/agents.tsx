// Agentes: cor validada para daltonismo, ordem da pilha (de baixo para cima) e logo oficial do desktop nativo.
import antigravity from "../../../../../desktop-native/assets/providers/antigravity.svg?raw";
import claude from "../../../../../desktop-native/assets/providers/claude.svg?raw";
import codex from "../../../../../desktop-native/assets/providers/codex.svg?raw";
import kimi from "../../../../../desktop-native/assets/providers/kimi.svg?raw";
import omp from "../../../../../desktop-native/assets/providers/omp.svg?raw";
import pi from "../../../../../desktop-native/assets/providers/pi.svg?raw";
import { AGENTS } from "./labels";

export const AGENT_ORDER = ["claude", "pi", "codex", "kimi", "omp"] as const;
export type AgentKey = (typeof AGENT_ORDER)[number] | "outro";
export const AGENT_COLOR: Record<AgentKey, string> = { claude: "#d97757", pi: "#8b5cf6", codex: "#10a37f", kimi: "#4f8fe0", omp: "#c08a1c", outro: "#857c81" };
export const agentKey = (p: unknown): AgentKey => (AGENT_ORDER as readonly string[]).includes(p as string) ? (p as AgentKey) : "outro";
export const agentName = (k: string) => (k === "outro" ? "Outros" : AGENTS[k] ?? k);

const path = (svg: string) => svg.match(/ d="([^"]+)"/)?.[1] ?? "";
const LOGO: Record<string, string> = { claude: path(claude), codex: path(codex), pi: path(pi), kimi: path(kimi), omp: path(omp), antigravity: path(antigravity) };

export function AgentLogo({ provider, size = 16, color }: { provider: string; size?: number; color?: string }) {
  const d = LOGO[provider];
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill={color ?? AGENT_COLOR[agentKey(provider)]} aria-hidden="true" className="shrink-0">
      {d ? <path d={d} /> : <circle cx="12" cy="12" r="6" />}
    </svg>
  );
}
