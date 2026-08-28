// Orquestração: quem roda cada papel do grupo (contrato `regras-<gid>.md`) e quais contas a
// política da máquina libera (`orquestracao-contas.md`). Tipos das rotas + a comparação
// contrato × medido, que tem TRÊS estados: o contrato guarda id (`opus[1m]`), a statusline traz
// rótulo (`Opus4.8·1M`), e sessão sem statusline não é "diferente" — é "não medido".
// (`lib/orq.ts` é outra coisa: a retrospectiva das execuções.)
import { parseStatusLine } from './statusline';
import type { SessionInfo } from './types';

export type Provider = 'claude' | 'codex' | 'pi' | 'kimi';

export interface Papel {
  papel: string;
  sessao: string;          // nome exato ou prefixo com `*` no fim (`trab-t*`)
  provider: Provider | '';
  conta: string;
  modelo: string;
  esforco: string;
  // '' = o papel roda sempre na mesma conta. '1','2','3'… = rodízio, e a Task N cabe à conta de
  // índice (N-1) % total — regra determinística, sem estado guardado. 'par' = todas ao mesmo tempo.
  vez?: string;
  viva: string | null;     // nome da sessão viva casada pelo backend, ou null
  id_cota?: string | null; // chave do /api/cotas da conta do papel
}

// Dois modos, e não três: "rodar Tasks em paralelo" é outra coisa e já existe na skill (Tasks
// independentes, uma worktree cada, cada uma com seu executor e seu revisor). Aquilo se declara no
// PLANO — ver references/paralelo-worktree.md —, não na configuração de um papel.
export type ModoPapel = 'unica' | 'reveza';

/** Agrupa as linhas de um mesmo papel: o contrato tem uma linha por conta quando há rodízio. */
export function agruparPorPapel(papeis: Papel[]): { papel: string; linhas: Papel[]; modo: ModoPapel }[] {
  const ordem: string[] = [];
  const mapa = new Map<string, Papel[]>();
  for (const p of papeis) {
    const k = p.papel.trim().toLowerCase();
    if (!mapa.has(k)) { mapa.set(k, []); ordem.push(k); }
    mapa.get(k)!.push(p);
  }
  return ordem.map((k) => {
    const linhas = mapa.get(k)!;
    // O modo sai do CONTEÚDO, não de um campo à parte: uma linha é conta única, várias é rodízio.
    // Assim não há como o modo discordar das linhas.
    const modo: ModoPapel = linhas.length < 2 ? 'unica' : 'reveza';
    return { papel: linhas[0].papel, linhas, modo };
  });
}

/** De quem é a vez na Task N (1-based). Round-robin puro: sem estado, derivável a qualquer momento. */
export function contaDaTask(linhas: Papel[], task: number): Papel | null {
  if (!linhas.length) return null;
  return linhas[((task - 1) % linhas.length + linhas.length) % linhas.length];
}

export interface RespostaPapel {
  papel: Omit<Papel, 'viva' | 'id_cota'>;
  papeis: Omit<Papel, 'viva' | 'id_cota'>[];
  mtime: number;
  arbitro: string | null;
  // `nao_avisado` = gravou com `avisar: false` (o "salvar e continuar"); não é falha, é o árbitro
  // deliberadamente não acordado ainda.
  aviso: 'enviado' | 'enfileirado' | 'sem_arbitro' | 'falhou' | 'nao_avisado';
  erro: string | null;
}

export interface OrqGrupo {
  gid: string;
  arquivo: string;
  mtime: number;
  papeis: Papel[];
  arbitro: string | null;
}

// Conta que está na tabela "O que pode" = liberada. Fora dela = proibida (não existe "desligada
// mas listada" no arquivo; a tela deriva o interruptor da presença).
export interface ContaPolitica {
  conta: string;
  provider: Provider;
  apelido: string;
  modelos: string[];       // ids; ['*'] = todos do catálogo
  trocar: boolean;
}

export interface ModeloInventario {
  id: string;
  name?: string;
  context_length?: number | string | null;   // Pi manda texto ("200k"), Kimi/motor número
  efforts?: string[];
}

export interface ContaInventario {
  conta: string;
  provider: Provider;
  apelido: string;
  id_cota: string | null;  // chave do /api/cotas e dos apelidos
  modelos: ModeloInventario[];
  reduced: boolean;        // Claude sem sessão viva: só os 3 aliases
}

// Política VAZIA (arquivo sem tabela) = nada proibido ainda: o inventário inteiro vale, com `*`.
// A regra "só o que a política permite" passa a valer na primeira conta ligada/desligada.
export const politicaVazia = (pol: ContaPolitica[]) => pol.length === 0;

export const politicaDe = (pol: ContaPolitica[], provider: string, conta: string, inv?: ContaInventario[]): ContaPolitica | null => {
  const achada = pol.find((c) => c.provider === provider && c.conta === conta) ?? null;
  if (achada || !politicaVazia(pol)) return achada;
  const i = inv?.find((c) => c.provider === provider && c.conta === conta);
  return i ? { conta: i.conta, provider: i.provider, apelido: i.apelido, modelos: ['*'], trocar: true } : null;
};

export const contasLiberadas = (pol: ContaPolitica[], inv: ContaInventario[], provider: string): ContaPolitica[] =>
  politicaVazia(pol)
    ? inv.filter((c) => c.provider === provider).map((c) => ({ conta: c.conta, provider: c.provider, apelido: c.apelido, modelos: ['*'], trocar: true }))
    : pol.filter((c) => c.provider === provider);

// Modelos que a política deixa escolher nesta conta: `*` abre o catálogo inteiro; lista fechada
// mantém a ordem do catálogo e acrescenta no fim o id digitado que o catálogo (reduzido) não tem.
export function modelosLiberados(inv: ContaInventario | null, pol: ContaPolitica | null): ModeloInventario[] {
  if (!pol) return [];
  const cat = inv?.modelos ?? [];
  if (pol.modelos.includes('*')) return cat;
  const ids = new Set(pol.modelos);
  const doCatalogo = cat.filter((m) => ids.has(m.id));
  const vistos = new Set(doCatalogo.map((m) => m.id));
  return [...doCatalogo, ...pol.modelos.filter((id) => !vistos.has(id)).map((id) => ({ id }))];
}

export const iniciais = (nome: string) =>
  nome.split(/[\s_\-:/]+/).filter((p) => p && !/^(e|and|de|da|do|&)$/i.test(p))
    .slice(0, 2).map((p) => p[0]!.toUpperCase()).join('') || '?';

export interface OrqPolitica {
  politica: ContaPolitica[];
  inventario: ContaInventario[];
  arquivo: string;
  mtime: number;
}

export type Divergencia = 'igual' | 'divergente' | 'nao_medido';

// Família do modelo, o que sobrevive entre id e rótulo: `opus[1m]` e `Opus4.8·1M` são ambos
// {familia: 'opus', um: true}. Provider do Pi/Kimi (`apikey/k3`) sai da comparação.
export function familiaDe(modelo: string): { familia: string; um: boolean } | null {
  const s = modelo.trim().toLowerCase();
  if (!s) return null;
  const um = /\[1m\]|·1m\b|\b1m\b|1m$/.test(s);
  const semProvider = s.includes('/') ? s.slice(s.lastIndexOf('/') + 1) : s;
  const m = semProvider.match(/[a-z]+/);
  if (!m) return null;
  return { familia: m[0], um };
}

const ESFORCO_ALIAS: Record<string, string> = { med: 'medium', min: 'minimal' };
const normEsforco = (e: string) => {
  const s = e.trim().toLowerCase().replace(/[^a-z]/g, '');
  return ESFORCO_ALIAS[s] ?? s;
};

export function compararModelo(contrato: string, medido: string | null | undefined): Divergencia {
  if (!medido) return 'nao_medido';
  const a = familiaDe(contrato);
  const b = familiaDe(medido);
  if (!a || !b) return 'nao_medido';
  return a.familia === b.familia && a.um === b.um ? 'igual' : 'divergente';
}

export function compararEsforco(contrato: string, medido: string | null | undefined): Divergencia {
  if (!medido) return 'nao_medido';
  if (!contrato) return 'igual';
  return normEsforco(contrato) === normEsforco(medido) ? 'igual' : 'divergente';
}

// Conta medida só existe pro Claude (`claude:<dir absoluto>`, lido do /proc). Pi nunca tem, e a
// do Kimi vem do default_model global — mente por sessão. Os dois: não medido.
export function compararConta(papel: Papel, sessao: SessionInfo): Divergencia {
  if (papel.provider !== 'claude' || !sessao.conta || !sessao.conta.startsWith('claude:')) return 'nao_medido';
  const dir = sessao.conta.slice('claude:'.length).replace(/\/+$/, '');
  const nome = dir.split('/').pop() ?? '';
  const alvo = papel.conta === 'padrao' ? '.claude' : `.claude-${papel.conta}`;
  return nome === alvo || nome === papel.conta ? 'igual' : 'divergente';
}

// Nome curto da conta medida, na mesma grafia do contrato (`padrao`, `200-01`, `apikey`).
export function contaMedida(sessao: SessionInfo): string | null {
  const id = sessao.conta;
  if (!id) return null;
  if (id.startsWith('claude:')) {
    const nome = id.slice('claude:'.length).replace(/\/+$/, '').split('/').pop() ?? '';
    return nome === '.claude' ? 'padrao' : nome.replace(/^\.claude-/, '');
  }
  return id.split(':').slice(1).join(':') || null;
}

export interface EstadoPapel {
  viva: boolean;
  modeloMedido: string | null;
  esforcoMedido: string | null;
  contaMedida: string | null;
  modelo: Divergencia;
  esforco: Divergencia;
  conta: Divergencia;
  divergente: boolean;
}

export function estadoDoPapel(papel: Papel, sessao: SessionInfo | null): EstadoPapel {
  if (!sessao) {
    return { viva: false, modeloMedido: null, esforcoMedido: null, contaMedida: null,
             modelo: 'nao_medido', esforco: 'nao_medido', conta: 'nao_medido', divergente: false };
  }
  const st = parseStatusLine(sessao.status_line);
  const modelo = compararModelo(papel.modelo, st?.model);
  const esforco = compararEsforco(papel.esforco, st?.effort);
  const conta = compararConta(papel, sessao);
  return {
    viva: true,
    modeloMedido: st?.model ?? null,
    esforcoMedido: st?.effort ?? null,
    contaMedida: contaMedida(sessao),
    modelo, esforco, conta,
    divergente: [modelo, esforco, conta].includes('divergente'),
  };
}

// Casa o nome do contrato com uma sessão viva: exato, ou prefixo com `*` no fim → a mais recente.
export function casarViva(sessao: string, vivas: SessionInfo[]): SessionInfo | null {
  if (!sessao) return null;
  if (!sessao.endsWith('*')) return vivas.find((s) => s.name === sessao) ?? null;
  const prefixo = sessao.slice(0, -1);
  return vivas
    .filter((s) => s.name.startsWith(prefixo))
    .sort((a, b) => (b.last_activity ?? 0) - (a.last_activity ?? 0))[0] ?? null;
}
