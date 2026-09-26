// Relatório de uso (skills, tools, Bash, MCP, agentes, contexto injetado, plugins) do Claude
// Code, lido do transcript pelo backend (`/api/uso`). Irmão do relatório de custos: mesma
// malha de servidores, mesma regra de mescla (soma por chave, servidor antigo tolerado na
// ENTRADA), mesmos avisos de parcial/desatualizado.
import type { Applied } from './types';

export interface UsoBucket {
  key: string;
  label?: string | null;
  plugin?: string;
  sessions: number;
  chamadas: number;
  // Das chamadas, quantas o usuário pediu (skill por barra: exato; agente: heurística do prompt).
  pedidas: number;
  ctx_chars: number;
  // Estimativa (chars/4) do que entrou no contexto. Nunca vira dólar; a tela rotula "≈".
  ctx_tokens_est: number;
  // Reais só onde há `usage` por trás: skill (respostas da execução) e agente (transcript filho).
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
  cost: number;
  cost_input: number;
  cost_output: number;
  cost_cache_write: number;
  cost_cache_read: number;
  // Skill/plugin: tokens que o texto ocupou (tamanho × respostas no contexto) e quantas respostas.
  // Opcionais: servidor antigo da malha não manda.
  ocupados_tokens_est?: number;
  // O mesmo pesado pelo que cada resposta pagou (releitura de cache vale 0,1 do token novo).
  ocupados_eq_tokens_est?: number;
  respostas?: number;
  // Sessões de subagente, fora de `sessions`.
  subagentes?: number;
}

export type UsoDim = 'by_skill' | 'by_tool' | 'by_bash' | 'by_mcp' | 'by_agente' | 'by_contexto' | 'by_imagem'
  | 'by_area' | 'by_area_dia' | 'by_plugin' | 'by_conta' | 'by_projeto' | 'by_modelo';
export const USO_DIMS: UsoDim[] = ['by_skill', 'by_tool', 'by_bash', 'by_mcp', 'by_agente', 'by_contexto', 'by_imagem',
  'by_area', 'by_area_dia',
  'by_plugin', 'by_conta', 'by_projeto', 'by_modelo'];

// Filtros que vão ao servidor, repetíveis (`?conta=a&conta=b&projeto=…&foco=`); lista vazia =
// todos. `foco` só recorta a série diária: é o clique numa linha da tabela.
export interface UsoFiltros {
  conta?: string[];
  projeto?: string[];
  modelo?: string[];
  plugin?: string[];
  foco?: string;
}

export interface UsoReport {
  totals: UsoBucket;
  by_skill: UsoBucket[];
  by_tool: UsoBucket[];
  by_bash: UsoBucket[];
  by_mcp: UsoBucket[];
  by_agente: UsoBucket[];
  by_contexto: UsoBucket[];
  // `enviada` (imagem no prompt) e `lida:<tool>` (Read num PNG, print); tokens pelos pixels.
  by_imagem: UsoBucket[];
  // Área do código (front/back/banco/…/conversa): custo REAL do turno dividido pelas tools de
  // arquivo. A série é dia × área: key `YYYY-MM-DD|área`, label = área. Servidor antigo: vazio.
  by_area: UsoBucket[];
  by_area_dia: UsoBucket[];
  by_plugin: UsoBucket[];
  // Listas dos seletores, do período inteiro SEM os filtros de dimensão; os campos soltos ecoam
  // o filtro aplicado.
  by_conta: UsoBucket[];
  by_projeto: UsoBucket[];
  by_modelo: UsoBucket[];
  // Série diária (key = YYYY-MM-DD) sob os filtros (e sob `foco`, se houver).
  by_day: UsoBucket[];
  conta?: string[];
  projeto?: string[];
  modelo?: string[];
  plugin?: string[];
  foco?: string | null;
  applied?: Applied | null;
  usd_brl?: number | null;
}

export interface UsoServerResult {
  report: Partial<UsoReport> | null;
  label?: string;
  id?: string;
}

export interface MergedUso {
  report: UsoReport & { by_servidor: UsoBucket[] };
  partial: boolean;
  mismatched: string[];
  failed: string[];
}

export const zeroUso = (key: string): UsoBucket => ({
  key, label: null, plugin: '', sessions: 0, chamadas: 0, pedidas: 0, ctx_chars: 0, ctx_tokens_est: 0,
  input: 0, output: 0, cache_write: 0, cache_read: 0, cost: 0, ocupados_tokens_est: 0, ocupados_eq_tokens_est: 0, respostas: 0, subagentes: 0,
  cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0,
});

// `?? 0` em tudo: servidor antigo da malha sem um campo não pode virar NaN na coluna inteira.
function somar(alvo: UsoBucket, b: Partial<UsoBucket>): void {
  alvo.sessions += b.sessions ?? 0;
  alvo.chamadas += b.chamadas ?? 0;
  alvo.pedidas += b.pedidas ?? 0;
  alvo.ctx_chars += b.ctx_chars ?? 0;
  alvo.ctx_tokens_est += b.ctx_tokens_est ?? 0;
  alvo.input += b.input ?? 0;
  alvo.output += b.output ?? 0;
  alvo.cache_write += b.cache_write ?? 0;
  alvo.cache_read += b.cache_read ?? 0;
  alvo.cost += b.cost ?? 0;
  alvo.cost_input += b.cost_input ?? 0;
  alvo.cost_output += b.cost_output ?? 0;
  alvo.cost_cache_write += b.cost_cache_write ?? 0;
  alvo.cost_cache_read += b.cost_cache_read ?? 0;
  alvo.ocupados_tokens_est = (alvo.ocupados_tokens_est ?? 0) + (b.ocupados_tokens_est ?? 0);
  alvo.ocupados_eq_tokens_est = (alvo.ocupados_eq_tokens_est ?? 0) + (b.ocupados_eq_tokens_est ?? 0);
  alvo.respostas = (alvo.respostas ?? 0) + (b.respostas ?? 0);
  alvo.subagentes = (alvo.subagentes ?? 0) + (b.subagentes ?? 0);
}

function juntar(destino: Map<string, UsoBucket>, lista: UsoBucket[] | undefined): void {
  for (const b of lista ?? []) {
    if (!b || typeof b.key !== 'string') continue;
    let alvo = destino.get(b.key);
    if (!alvo) { alvo = zeroUso(b.key); destino.set(b.key, alvo); }
    alvo.label = alvo.label ?? b.label ?? null;
    alvo.plugin = alvo.plugin || b.plugin || '';
    somar(alvo, b);
  }
}

const ordenar = (m: Map<string, UsoBucket>) =>
  [...m.values()].sort((a, b) =>
    b.cost - a.cost || b.ctx_chars - a.ctx_chars || b.chamadas - a.chamadas || a.key.localeCompare(b.key));

export interface UsoItemGrupo { nome: string; keys: string[]; peso: number; vezes: number }
export interface UsoGrupo { plugin: string; peso: number; vezes: number; itens: UsoItemGrupo[] }

// Grupo = `plugin` do servidor, senão o prefixo da chave, senão `semPlugin`. O item perde o
// prefixo e junta o mesmo nome vindo do Claude e do Codex (`superpowers:brainstorming` e
// `brainstorming`); `keys` guarda as chaves originais, a mais pesada primeiro (é a que o detalhe abre).
export function agruparPorPlugin(lista: UsoBucket[], peso: (b: UsoBucket) => number, semPlugin = ''): UsoGrupo[] {
  const grupos = new Map<string, UsoGrupo>();
  for (const b of [...lista].sort((x, y) => peso(y) - peso(x) || y.chamadas - x.chamadas)) {
    const i = b.key.indexOf(':');
    const plugin = b.plugin || (i > 0 ? b.key.slice(0, i) : semPlugin);
    const nome = i > 0 ? b.key.slice(i + 1) : b.key;
    let g = grupos.get(plugin);
    if (!g) { g = { plugin, peso: 0, vezes: 0, itens: [] }; grupos.set(plugin, g); }
    let item = g.itens.find((x) => x.nome === nome);
    if (!item) { item = { nome, keys: [], peso: 0, vezes: 0 }; g.itens.push(item); }
    item.keys.push(b.key);
    item.peso += peso(b);
    item.vezes += b.chamadas;
    g.peso += peso(b);
    g.vezes += b.chamadas;
  }
  const lista2 = [...grupos.values()].filter((g) => g.peso > 0 || g.vezes > 0);
  for (const g of lista2) g.itens.sort((a, b) => b.peso - a.peso || b.vezes - a.vezes || a.nome.localeCompare(b.nome));
  return lista2.sort((a, b) => b.peso - a.peso || b.vezes - a.vezes || a.plugin.localeCompare(b.plugin));
}

export function mergeUso(results: UsoServerResult[], period: string): MergedUso {
  const totals = zeroUso('totals');
  const dims = Object.fromEntries(USO_DIMS.map((d) => [d, new Map<string, UsoBucket>()])) as Record<UsoDim, Map<string, UsoBucket>>;
  const dias = new Map<string, UsoBucket>();
  const servidores: UsoBucket[] = [];
  const mismatched: string[] = [];
  const failed: string[] = [];
  let partial = false;
  let usdBrl: number | null = null;

  results.forEach((res, i) => {
    const r = res.report;
    if (!r) { partial = true; failed.push(res.label ?? `#${i + 1}`); return; }
    usdBrl = usdBrl ?? r.usd_brl ?? null;
    if ((r.applied?.period ?? null) !== period) {
      partial = true;
      mismatched.push(res.label ?? `#${i + 1}`);
      return;
    }
    somar(totals, r.totals ?? {});
    const bs = zeroUso(res.id ?? res.label ?? `#${i + 1}`);
    bs.label = res.label ?? null;
    somar(bs, r.totals ?? {});
    servidores.push(bs);
    for (const d of USO_DIMS) juntar(dims[d], r[d]);
    juntar(dias, r.by_day);
  });
  const primeiro = results.find((r) => r.report && (r.report.applied?.period ?? null) === period)?.report;
  const porUso = (m: Map<string, UsoBucket>) =>
    [...m.values()].sort((a, b) => b.cost - a.cost || b.chamadas - a.chamadas || a.key.localeCompare(b.key));

  return {
    report: {
      totals,
      by_skill: ordenar(dims.by_skill),
      by_tool: ordenar(dims.by_tool),
      by_bash: ordenar(dims.by_bash),
      by_mcp: ordenar(dims.by_mcp),
      by_agente: ordenar(dims.by_agente),
      by_contexto: ordenar(dims.by_contexto),
      by_imagem: ordenar(dims.by_imagem),
      by_area: porUso(dims.by_area),
      by_area_dia: [...dims.by_area_dia.values()].sort((a, b) => a.key.localeCompare(b.key)),
      by_plugin: ordenar(dims.by_plugin),
      by_conta: porUso(dims.by_conta),
      by_projeto: porUso(dims.by_projeto),
      by_modelo: porUso(dims.by_modelo),
      by_day: [...dias.values()].sort((a, b) => a.key.localeCompare(b.key)),
      by_servidor: servidores.sort((a, b) => b.cost - a.cost || b.chamadas - a.chamadas),
      // Os filtros são os mesmos pra malha inteira: o eco de qualquer servidor que entrou serve.
      conta: primeiro?.conta ?? [], projeto: primeiro?.projeto ?? [], modelo: primeiro?.modelo ?? [],
      plugin: primeiro?.plugin ?? [], foco: primeiro?.foco ?? null,
      applied: { period },
      usd_brl: usdBrl,
    },
    partial, mismatched, failed,
  };
}
