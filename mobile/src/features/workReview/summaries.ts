import { useSyncExternalStore } from 'react';
import type { WorkReviewSummary } from '@hangar/core';

// Fora do store da lista, como o status da worktree: a linha lê só o resumo dela, sem carregar o
// store inteiro (e o MMKV que vem junto) em cada lugar que desenha uma linha.
let all: Record<string, Record<string, WorkReviewSummary>> = {};
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((f) => f());
const subscribe = (f: () => void) => { listeners.add(f); return () => { listeners.delete(f); }; };

const same = (a: WorkReviewSummary, b: WorkReviewSummary) => a.review_id === b.review_id && a.state === b.state
  && a.updated_at === b.updated_at && a.badge?.text === b.badge?.text && a.badge?.tone === b.badge?.tone;

/** O evento traz o mapa inteiro do servidor; o resumo que não mudou mantém o objeto e não redesenha o chip. */
export function setWorkReviewSummaries(serverId: string, reviews: Record<string, WorkReviewSummary>) {
  const prev = all[serverId] ?? {};
  const next: Record<string, WorkReviewSummary> = {};
  for (const [name, summary] of Object.entries(reviews)) next[name] = prev[name] && same(prev[name], summary) ? prev[name] : summary;
  all = { ...all, [serverId]: next };
  notify();
}

export function clearWorkReviewSummaries() {
  if (Object.keys(all).length === 0) return;
  all = {};
  notify();
}

export function workReviewSummaries(): Record<string, Record<string, WorkReviewSummary>> { return all; }

export function useWorkReviewSummary(serverId: string, name: string): WorkReviewSummary | null {
  return useSyncExternalStore(subscribe, () => all[serverId]?.[name] ?? null);
}
