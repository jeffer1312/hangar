import type { StatusFields } from '@hangar/core';

export type Chip = {
  key: 'ctx' | 'cost' | '5h' | '7d' | '30d' | 'repo' | 'time';
  text: string;
  warn: boolean;
};

// Mesma escolha da PWA (ContextRing + Composer): só desenha o que veio; >= 80% vira aviso.
export function statusChips(f: StatusFields | null): Chip[] {
  if (!f) return [];
  const out: Chip[] = [];
  if (f.ctxPct != null) {
    out.push({ key: 'ctx', text: `${Math.round(f.ctxPct)}%`, warn: f.ctxPct >= 80 });
  }
  if (f.costUsd != null) {
    out.push({ key: 'cost', text: `$${f.costUsd.toFixed(2)}`, warn: false });
  }
  if (f.fiveHourPct != null) {
    out.push({ key: '5h', text: `5h ${f.fiveHourPct}%`, warn: f.fiveHourPct >= 80 });
  }
  if (f.weeklyPct != null) {
    out.push({ key: '7d', text: `7d ${f.weeklyPct}%`, warn: f.weeklyPct >= 80 });
  }
  if (f.monthlyPct != null) {
    out.push({ key: '30d', text: `30d ${f.monthlyPct}%`, warn: f.monthlyPct >= 80 });
  }
  if (f.branch) {
    out.push({
      key: 'repo',
      text: `${f.repo ?? ''} [${f.branch}${f.dirty ? '*' : ''}]`.trim(),
      warn: false,
    });
  }
  if (f.sessionTime) {
    out.push({ key: 'time', text: f.sessionTime, warn: false });
  }
  return out;
}
