import { abbrevNum, basename, cotaDoModeloAgy, rateLabel, resetsIn, type JanelaCotaResumo, type SessionInfo, type StatsEvent, type StatusFields } from '@hangar/core';
import * as m from '../paraglide/messages';

const fmtDur = (ms: number) =>
  ms < 60_000 ? `${Math.round(ms / 1000)}s` : `${Math.floor(ms / 60_000)}m${Math.round((ms % 60_000) / 1000)}s`;

// Mesma ordem da faixa da PWA; campo que não veio não vira parte vazia.
export function linhaStats(s: StatsEvent): string[] {
  const p = [
    s.turns === 1 ? m.stats_turnos_1() : m.stats_turnos({ n: s.turns }),
    s.steps === 1 ? m.stats_chamadas_1() : m.stats_chamadas({ n: s.steps }),
  ];
  if (s.llm_ms) {
    p.push(m.stats_llm({ t: fmtDur(s.llm_ms) }));
    if (s.tool_ms) p.push(m.stats_tools({ t: fmtDur(s.tool_ms) }));
  }
  if (s.tok_s) p.push(m.stats_toks({ n: Math.round(s.tok_s) }));
  if (s.tok_s_now) p.push(m.stats_toks_now({ n: rateLabel(s.tok_s_now, s.tok_s_exact) }));
  if (s.tok_s_recent) p.push(m.stats_toks_recent({ n: rateLabel(s.tok_s_recent, s.tok_s_exact) }));
  if (s.ttft_ms) p.push(m.stats_ttft({ t: fmtDur(s.ttft_ms) }));
  if (s.cache_pct != null) p.push(m.stats_cache({ n: s.cache_pct }));
  p.push(m.stats_io({ i: abbrevNum(s.in_tok), o: abbrevNum(s.out_tok) }));
  return p;
}

export type UsageWindow = { key: '5h' | '7d' | '30d'; label: string; pct: number; reset?: string };

const known = (n: unknown): n is number => typeof n === 'number' && isFinite(n);

// Janelas de cota da conta, na ordem da folha de Uso da PWA; só entra o que a statusline trouxe.
export function usageWindows(f: StatusFields | null): UsageWindow[] {
  if (!f) return [];
  const out: UsageWindow[] = [];
  if (known(f.fiveHourPct)) out.push({ key: '5h', label: m.uso_janela_5h(), pct: f.fiveHourPct, reset: f.fiveHourReset });
  if (known(f.weeklyPct)) out.push({ key: '7d', label: m.uso_janela_7d(), pct: f.weeklyPct, reset: f.weeklyReset });
  if (known(f.monthlyPct)) out.push({ key: '30d', label: m.uso_janela_30d(), pct: f.monthlyPct, reset: f.monthlyReset });
  return out;
}

// O agy não publica linha de status: a cota vem da leitura da conta, só o grupo do modelo da sessão.
export function agyUsageWindows(quota: { estado: string; janelas: JanelaCotaResumo[] } | null | undefined, model: string | null | undefined): UsageWindow[] {
  if (quota?.estado !== 'lida') return [];
  const windows = cotaDoModeloAgy(quota, model).janelas;
  const out: UsageWindow[] = [];
  for (const [key, label] of [['5h', m.uso_janela_5h()], ['7d', m.uso_janela_7d()]] as const) {
    const window = windows.find((w) => w.rotulo === key);
    if (window && known(window.pct)) out.push({ key, label, pct: window.pct, reset: resetsIn(window.reset_ts) || undefined });
  }
  return out;
}

// Rótulo do botão único do composer: "Opus5.5·1M · high". Sem modelo lido, cai no nome do campo.
export function settingsLabel(model: string | null | undefined, effort: string | null | undefined): string {
  if (!model) return m.composer_modelo();
  return effort ? `${spacedModel(model)} · ${effort}` : spacedModel(model);
}

// "Opus5.5·1M" → "Opus 5.5 · 1M": a statusline cola tudo; no chip do composer o nome tem de ler
// como no app de PC. Duas letras antes do número: "o3" é nome, não "o 3".
export function spacedModel(model: string): string {
  return model.replace(/([A-Za-z]{2})(\d)/g, '$1 $2').replace(/\s*·\s*/g, ' · ').trim();
}

export type ComposerStatus = {
  folder?: string;
  branch?: string;
  added?: number;
  removed?: number;
  time?: string;
  ctxPct?: number;
  quotas?: UsageWindow[];
  cost?: string;
};

export function fitComposerItems(
  items: { key: string; width: number; shrink?: boolean }[], availableWidth: number, gap = 4,
): Record<string, number> {
  const widths = Object.fromEntries(items.map(item => [item.key, item.shrink ? 44 : Math.max(44, item.width)]));
  const occupied = () => Object.values(widths).reduce((sum, width) => sum + width, 0)
    + Math.max(0, Object.keys(widths).length - 1) * gap;
  for (const key of ['time', 'diff', 'folder', 'branch', 'cost', 'ctx', '30d', '7d', '5h', 'account', 'mode', ...Object.keys(widths)]) {
    if (occupied() <= availableWidth) break;
    delete widths[key];
  }
  let spare = Math.max(0, availableWidth - occupied());
  for (const key of ['mode', 'account', 'branch', 'folder']) {
    const item = items.find(item => item.key === key);
    if (!item?.shrink || widths[key] == null) continue;
    const extra = Math.min(spare, Math.max(0, item.width - widths[key]));
    widths[key] += extra;
    spare -= extra;
  }
  return widths;
}

type GitSession = Pick<SessionInfo, 'cwd' | 'branch' | 'git_added' | 'git_removed'>;

// Linha de status embaixo do composer (a do app de PC): o que a statusline trouxe, e da lista o
// que ela não traz (pasta, branch e o diff). Campo ausente não vira parte vazia.
export function composerStatus(f: StatusFields | null, session: GitSession | null): ComposerStatus {
  const out: ComposerStatus = {};
  const folder = f?.repo ?? (session?.cwd ? basename(session.cwd) : undefined);
  if (folder) out.folder = folder;
  const branch = f?.branch ?? session?.branch ?? undefined;
  if (branch) out.branch = branch;
  const added = session?.git_added;
  const removed = session?.git_removed;
  if (known(added) && added > 0) out.added = added;
  if (known(removed) && removed > 0) out.removed = removed;
  if (f?.sessionTime) out.time = f.sessionTime;
  const ctx = f?.ctxPct;
  if (known(ctx)) out.ctxPct = ctx;
  const quotas = usageWindows(f);
  if (quotas.length) out.quotas = quotas;
  const cost = f?.costUsd;
  if (known(cost)) out.cost = `$${cost.toFixed(2)}`;
  return out;
}
