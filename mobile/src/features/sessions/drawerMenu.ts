import { cotaDaConta, resumoCota } from '@hangar/core';
import type { AggSession, CotaContaResumo } from '@hangar/core';

export function recentSessions(rows: AggSession[], limit: number): AggSession[] {
  return rows
    .filter((r) => r.state !== 'dead' && !r.hidden_from_list)
    .sort((a, b) => (b.last_activity ?? -Infinity) - (a.last_activity ?? -Infinity))
    .slice(0, limit);
}

// Linha de "Contas" no menu: só a conta ativa, para caber numa linha.
export function accountQuotaLine(configs: { path: string; label: string; active?: boolean }[], cotas: CotaContaResumo[]): string {
  const active = configs.find((c) => c.active);
  if (!active) return '';
  return [active.label, resumoCota(cotaDaConta(cotas, active.path))].filter(Boolean).join(' · ');
}
