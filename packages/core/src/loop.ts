// Helpers puros do loop runner: badge de status (Task 11 consome — Sidebar/SessionList/BoardCard/Canvas).

import type { LoopState } from './types';

export type LoopTone = 'ok' | 'warn' | 'attention' | 'muted';

export interface LoopBadge {
  label: string;
  tone: LoopTone;
}

const TONE_BY_STATUS: Record<LoopState['status'], LoopTone> = {
  running: 'ok',
  done: 'ok',
  paused_awaiting: 'attention',
  done_claimed: 'attention',
  exhausted: 'warn',
  failed: 'warn',
  stopped: 'muted',
};

// Tone -> cor, mesmo vocabulário dos badges de estado (awaiting = --warning, erro = --error,
// ok = accent, parado = muted). Canônico: SessionCard/BoardCard/Sidebar/LoopSheet importam daqui.
export const LOOP_TONE_COLOR: Record<LoopTone, string> = {
  ok: 'var(--accent)',
  warn: 'var(--error)',
  attention: 'var(--warning)',
  muted: 'var(--text-muted)',
};

// null = sem loop (status ausente) — o chamador esconde o badge. Estados finais silenciosos não
// existem hoje (todo status conhecido tem tom); mantém a assinatura null-safe pro Task 11.
export function loopBadge(
  status: LoopState['status'] | null | undefined,
  iter?: number | null,
  max?: number | null,
): LoopBadge | null {
  if (!status) return null;
  const tone = TONE_BY_STATUS[status];
  if (!tone) return null;
  // ↻ tipografico (nao emoji): monocromatico, herda a cor do tone — o 🔁 virava o emoji azul
  // brilhante do iOS e brigava com o resto da UI (feedback real de celular).
  const label = iter != null && max != null ? `↻ ${iter}/${max}` : '↻';
  return { label, tone };
}
