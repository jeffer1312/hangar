import type { ChatEvent, SubagentRun } from '@hangar/core';

// Rótulo do terminal partido em texto e tempo: "Bash: ls … (5m 11s · ↓ 5k tokens)" →
// { text: "Bash: ls …", elapsed: "5m 11s" }. O tempo do terminal vale mesmo abrindo a conversa no
// meio do turno, quando o envio que o começou já saiu da janela carregada.
const STATS = /\s\(([^()]*)\)\s*$/;
const ELAPSED = /^\d+[hms](?:\s\d+[hms])*$/;

export function workingLabel(label: string | null | undefined): { text: string | null; elapsed: string | null; detail: string | null } {
  const raw = label?.trim();
  if (!raw) return { text: null, elapsed: null, detail: null };
  const stats = STATS.exec(raw);
  const parts = stats?.[1].split(' · ').map((p) => p.trim()).filter((p) => p && !p.startsWith('esc to ')) ?? [];
  const elapsed = parts[0] && ELAPSED.test(parts[0]) ? parts[0] : null;
  const text = (stats ? raw.slice(0, stats.index) : raw).trim();
  // Tokens e o resto que o terminal mostra depois do tempo ("↓ 6.0k tokens · thought for 9s").
  const detail = (elapsed ? parts.slice(1) : parts).join(' · ');
  return { text: text || null, elapsed, detail: detail || null };
}

// Começo do turno (ms epoch): o mais recente entre o último envio gravado e a virada vista ao vivo.
// Sem nenhum dos dois não há o que contar.
export function turnStart(events: ChatEvent[], seenMs: number | null): number | null {
  let sent: number | null = null;
  for (let i = events.length - 1; i >= 0; i--) {
    const ev = events[i];
    if (ev.kind === 'user_msg' && !ev.id.startsWith('queued-') && ev.ts) { sent = ev.ts * 1000; break; }
  }
  if (sent === null) return seenMs;
  return seenMs === null ? sent : Math.max(sent, seenMs);
}

// O subagente ainda roda? Kimi e Pi dizem no arquivo do filho (`finished`); no Claude vale o
// transcript do pai. null = não dá pra saber (ilegível, ou só o disco o conhece).
export function subagentRunning(detail: Pick<SubagentRun, 'finished' | 'ilegivel'>, paiRodando: boolean | null): boolean | null {
  if (detail.ilegivel) return null;
  return detail.finished !== undefined ? !detail.finished : paiRodando;
}
