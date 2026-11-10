import type { Credencial } from './credenciais';

// A lista única de Contas: filtro por agente, busca e ordenação. O app de desktop (Rust) segue as mesmas regras
// em `desktop-native/src/app/accounts.rs`; mudou aqui, muda lá.

export type AccountFilter = 'all' | 'claude' | 'codex' | 'other';
/** `name`, `reset` (instante do reset da janela de 7 dias) ou o rótulo de uma janela de cota (`5h`, `7d`, `Fable`…). */
export type AccountSortKey = string;
export type AccountSort = { key: AccountSortKey; dir: 1 | -1 } | null;

export const ACCOUNT_FILTERS: AccountFilter[] = ['all', 'claude', 'codex', 'other'];

export function accountFilterOf(c: Pick<Credencial, 'tipo'>): Exclude<AccountFilter, 'all'> {
  return c.tipo === 'claude' ? 'claude' : c.tipo === 'codex' ? 'codex' : 'other';
}

export function accountInFilter(c: Pick<Credencial, 'tipo'>, filter: AccountFilter): boolean {
  return filter === 'all' || accountFilterOf(c) === filter;
}

/** Busca sem acento e sem caixa no nome, no nome natural e no email do login. */
export function accountMatches(c: Pick<Credencial, 'nome' | 'nome_natural' | 'login'>, term: string): boolean {
  const t = fold(term.trim());
  if (!t) return true;
  return [c.nome, c.nome_natural, c.login?.email ?? ''].some((s) => fold(s).includes(t));
}

/** Minúsculas e sem acento, para a busca achar "Aparência" digitando "aparencia". */
export function fold(s: string): string {
  return s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
}

/** Trecho de `text` que a busca achou, em posições do texto original (para destacar); null sem busca ou sem achado. */
export function accountMatchRange(text: string, term: string): [number, number] | null {
  const t = fold(term.trim());
  if (!t) return null;
  // Cada caractere dobrado lembra de qual caractere do original veio: dobrar pode mudar o comprimento.
  let folded = '';
  const origin: number[] = [];
  let i = 0;
  for (const ch of text) {
    const f = fold(ch);
    folded += f;
    for (let k = 0; k < f.length; k++) origin.push(i);
    i += ch.length;
  }
  const at = folded.indexOf(t);
  if (at < 0) return null;
  const last = origin[at + t.length - 1];
  return [origin[at], last + String.fromCodePoint(text.codePointAt(last)!).length];
}

/** Valor da conta na coluna; null = sem leitura, que fica no fim nos dois sentidos. */
export function accountSortValue(c: Pick<Credencial, 'nome' | 'cota'>, key: AccountSortKey): number | string | null {
  if (key === 'name') return fold(c.nome);
  const cota = c.cota?.estado === 'lida' ? c.cota : null;
  if (!cota) return null;
  const janela = cota.janelas.find((j) => j.rotulo === (key === 'reset' ? '7d' : key));
  if (!janela) return null;
  const v = key === 'reset' ? janela.reset_ts : janela.pct;
  return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

/** Sem ordem escolhida: as contas em uso primeiro, o resto na ordem recebida. Empate mantém a ordem recebida. */
export function sortAccounts<T extends Pick<Credencial, 'nome' | 'cota' | 'ativa'>>(list: T[], sort: AccountSort): T[] {
  const indexed = list.map((c, i) => ({ c, i }));
  // Ordem guardada por uma coluna que nenhuma conta tem agora (janela de outra conta, filtro): vale a padrão.
  const key = sort?.key;
  if (key && key !== 'name' && !list.some((c) => accountSortValue(c, key) != null)) sort = null;
  if (!sort) return indexed.sort((a, b) => Number(b.c.ativa) - Number(a.c.ativa) || a.i - b.i).map((x) => x.c);
  return indexed.sort((a, b) => {
    const x = accountSortValue(a.c, sort.key);
    const y = accountSortValue(b.c, sort.key);
    if (x == null || y == null) return x == null && y == null ? a.i - b.i : x == null ? 1 : -1;
    return x < y ? -sort.dir : x > y ? sort.dir : a.i - b.i;
  }).map((x) => x.c);
}

/** Nome e reset começam crescente; percentuais começam por quem gastou mais. Terceiro clique volta ao padrão. */
export function nextAccountSort(current: AccountSort, key: AccountSortKey): AccountSort {
  const first: 1 | -1 = key === 'name' || key === 'reset' ? 1 : -1;
  if (current?.key !== key) return { key, dir: first };
  return current.dir === first ? { key, dir: (-first) as 1 | -1 } : null;
}

export function parseAccountSort(raw: string | null | undefined): AccountSort {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw) as { key?: unknown; dir?: unknown };
    return typeof v.key === 'string' && (v.dir === 1 || v.dir === -1) ? { key: v.key, dir: v.dir } : null;
  } catch {
    return null;
  }
}
