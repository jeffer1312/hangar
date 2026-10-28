import * as m from './paraglide/messages';

// Resumo de cota de UMA conta pra caber numa linha de seletor ("5h 42% · 7d 18%"). Shape mínimo
// do /api/cotas (backend/app/cotas.py, `CotaConta`): o front web tem o tipo completo em
// lib/contaEstado; o app nativo só precisa disto.
export interface JanelaCotaResumo {
  rotulo: string;
  pct: number;
  reset_ts?: number | null;
  por_modelo?: boolean;
}

export interface CotaContaResumo {
  id: string;
  estado: 'lida' | 'sem_credencial' | 'expirada' | 'indisponivel';
  janelas: JanelaCotaResumo[];
  /** Código do backend (`sessao-viva`, `renovacao-falhou`, …), nunca texto de tela. */
  motivo?: string | null;
}

/** Números da última leitura boa com o `motivo` da falha nova: por que não atualizaram. */
export function motivoCotaAntiga(c: Pick<CotaContaResumo, 'estado' | 'motivo'> | null | undefined): string | null {
  const motivo = c?.estado === 'lida' ? c.motivo : null;
  const causa = motivo ? quotaFailureReason(motivo) : null;
  return causa ? m.cota_nao_atualizou({ causa }) : null;
}

export function quotaFailureReason(motivo: string): string | null {
  // Conta ocupada por login, renovação ou redefinição: não houve leitura que falhou.
  if (motivo === 'sessao-viva') return null;
  if (motivo === 'sem-resposta') return m.cota_causa_sem_resposta();
  if (motivo === 'http-429') return m.cota_causa_limite();
  if (motivo === 'http-401' || motivo === 'http-403') return m.cota_causa_recusou();
  const status = /^http-(\d{3})$/.exec(motivo)?.[1];
  // 2xx chega quando o corpo não serve (vazio, JSON que não é objeto): é formato, não erro da fonte.
  if (status && !status.startsWith('2')) return m.cota_causa_http({ status });
  if (status || ['formato-desconhecido', 'painel-sem-numeros', 'erro-leitor'].includes(motivo)) return m.cota_causa_formato();
  if (motivo === 'resposta-grande') return m.cota_causa_grande();
  if (motivo === 'credencial-invalida' || motivo === 'credencial-ilegivel') return m.cota_causa_credencial();
  return m.cota_causa_outra();
}

/** Leitura parou porque a credencial não renovou: resolve abrindo uma sessão na conta. */
export function cotaParada(c: CotaContaResumo | undefined): boolean {
  return c?.motivo === 'renovacao-falhou';
}

/** A cota da conta Claude cujo config dir é `path` — a chave do /api/cotas é `claude:<path>`. */
export function cotaDaConta<T extends { id: string }>(cotas: T[], path: string): T | undefined {
  return cotas.find((c) => c.id === `claude:${path}`);
}

/** Rótulo da primeira janela da conta inteira em 100% ("7d"); null se nenhuma estourou. A janela de
 *  um modelo só não trava a conta, por isso fica de fora. */
export function janelaEsgotada(c: CotaContaResumo | undefined): string | null {
  if (!c || c.estado !== 'lida') return null;
  return c.janelas.find((j) => !j.por_modelo && typeof j.pct === 'number' && j.pct >= 100)?.rotulo ?? null;
}

/** Uso a partir do qual a conta cede a vez: o `QUOTA_LIMIT` do servidor (session_open/resolve.rs). */
export const COTA_LIMITE = 95;

type CotaUso = { id: string; estado: string; janelas: { pct: number }[] };

/** Maior janela lida, de qualquer tipo (como o servidor conta); null sem leitura. */
function usoDaConta(c: CotaUso | undefined): number | null {
  const pcts = c?.estado === 'lida' ? c.janelas.map((j) => j.pct).filter((p) => typeof p === 'number' && isFinite(p)) : [];
  return pcts.length ? Math.max(...pcts) : null;
}

/** A regra do servidor ao abrir (`quota_verdict`), para a tela mostrar a conta em que a sessão vai nascer:
 *  a Claude `atual` a COTA_LIMITE% ou mais sai para a lida de menor uso, se ela tiver mais folga que a
 *  atual; empate fica com a ativa. null = fica onde está (abaixo do limite, sem leitura ou nenhuma melhor). */
export function contaComFolga(
  atual: string | null | undefined,
  contas: { path: string; active?: boolean }[],
  cotas: CotaUso[],
): string | null {
  const agora = atual ? usoDaConta(cotaDaConta(cotas, atual)) : null;
  if (agora === null || agora < COTA_LIMITE) return null;
  let best: { uso: number; ativa: boolean; path: string } | null = null;
  for (const conta of contas) {
    const uso = conta.path === atual ? null : usoDaConta(cotaDaConta(cotas, conta.path));
    if (uso === null || uso >= agora) continue;
    const ativa = !!conta.active;
    if (!best || uso < best.uso || (uso === best.uso && ativa && !best.ativa)) best = { uso, ativa, path: conta.path };
  }
  return best?.path ?? null;
}

/** "5h 42% · 7d 18%"; string vazia sem leitura ou sem janela (quem chama decide o texto). */
export function resumoCota(c: CotaContaResumo | undefined): string {
  if (!c || c.estado !== 'lida') return '';
  return c.janelas
    .filter((j) => typeof j.pct === 'number' && isFinite(j.pct))
    .map((j) => `${j.rotulo} ${Math.round(j.pct)}%`)
    .join(' · ');
}
