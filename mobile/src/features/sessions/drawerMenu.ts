import { cotaDaConta, resumoCota } from '@hangar/core';
import type { AggSession, CotaContaResumo } from '@hangar/core';

export function recentSessions(rows: AggSession[], limit: number): AggSession[] {
  // Como na lista: a de fora some junto com a sessão oculta a que está presa.
  const hidden = new Set(rows.filter((r) => r.hidden_from_list).map((r) => `${r.serverId}::${r.name}`));
  return rows
    .filter((r) => r.state !== 'dead' && !r.hidden_from_list && !(r.outside && hidden.has(`${r.outside.localServerId}::${r.outside.localSession}`)))
    .sort((a, b) => (b.last_activity ?? -Infinity) - (a.last_activity ?? -Infinity))
    .slice(0, limit);
}

// Linha de "Contas" no menu: só a conta ativa, para caber numa linha.
export function accountQuotaLine(configs: { path: string; label: string; active?: boolean }[], cotas: CotaContaResumo[]): string {
  const active = configs.find((c) => c.active);
  if (!active) return '';
  return [active.label, resumoCota(cotaDaConta(cotas, active.path))].filter(Boolean).join(' · ');
}
