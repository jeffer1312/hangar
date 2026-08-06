// A matemática do painel "Comparar": duas a quatro entidades da MESMA dimensão, lado a lado.
//
// Mora fora do .svelte pela mesma razão do `partirOcultos`: a garantia que estas funções dão
// ("cada balde é o recorte daquela chave" e "dia parado vale zero, não some do eixo") é
// justamente a candidata a *o total não bate*, e dentro do componente era intestável.
import { agruparPor, type DimFiltro } from './cubo';
import type { ComboLocal, DimBucket } from './types';

// Tokens é o padrão da tela, não o dinheiro: conta Anthropic e conta Kimi são assinatura/cota, e
// o `cost` é preço de tabela (models.dev). Comparar as duas em dólar responde a pergunta errada.
export type Metrica = 'tokens' | 'custo';

// Os quatro tipos somados. Vive aqui porque a mesma soma já estava copiada em três lugares.
// `?? 0` por campo: servidor antigo pode omitir um tipo de token, e soma crua vira NaN na série
// — o mesmo cuidado que `acumular`/`somarBucket` têm em todo o resto do módulo.
export const brutos = (b: { input: number; output: number; cache_write: number; cache_read: number }): number =>
  (b.input ?? 0) + (b.output ?? 0) + (b.cache_write ?? 0) + (b.cache_read ?? 0);

export const valorDe = (b: DimBucket, m: Metrica): number =>
  m === 'custo' ? b.cost : brutos(b);

// Segunda-feira da semana daquele dia, como 'YYYY-MM-DD'. Semana ISO de verdade (ano + número)
// não vale o preço: a chave só precisa ser estável, ordenável e rotulável, e uma data serve nas
// três com o formatador de dia que a tela já tem. Tudo em UTC — parseando 'T00:00:00Z' e usando
// getUTC*/setUTC*, o horário de verão nunca entra na conta.
export function segundaDe(dia: string): string {
  const d = new Date(`${dia}T00:00:00Z`);
  if (Number.isNaN(d.getTime())) return dia; // data malformada degrada, não derruba o painel
  d.setUTCDate(d.getUTCDate() - ((d.getUTCDay() + 6) % 7)); // 0 = segunda
  return d.toISOString().slice(0, 10);
}

function somaDias(x: string, n: number): string {
  const d = new Date(`${x}T00:00:00Z`);
  if (Number.isNaN(d.getTime())) return x; // idem — toISOString de Invalid Date LANÇA
  d.setUTCDate(d.getUTCDate() + n);
  return d.toISOString().slice(0, 10);
}

export interface Ponto {
  x: string;          // dia ('YYYY-MM-DD') ou a segunda da semana, no modo semanal
  valores: number[];  // um por chave, NA ORDEM de `chaves`
}

export function serieComparada(
  combos: ComboLocal[], dim: DimFiltro, chaves: string[], m: Metrica, semanal: boolean,
): Ponto[] {
  if (!chaves.length) return [];
  const pos = new Map(chaves.map((k, i) => [k, i]));
  const acc = new Map<string, number[]>();
  for (const c of combos) {
    const i = pos.get(c[dim] as string);
    if (i === undefined) continue;   // entidade não marcada não entra na régua do gráfico
    const x = semanal ? segundaDe(c.dia) : c.dia;
    let v = acc.get(x);
    // Zera as OUTRAS entidades no mesmo ponto: sem isto, um dia em que só a Kimi rodou deixaria a
    // linha da Anthropic sem valor ali, e o gráfico pularia o ponto em vez de encostar no chão.
    if (!v) { v = chaves.map(() => 0); acc.set(x, v); }
    v[i] += m === 'custo' ? (c.cost ?? 0) : brutos(c);
  }
  if (!acc.size) return [];
  // E os dias em que NINGUÉM rodou: sem eles o eixo espaça por índice e duas datas separadas por
  // uma semana ficam coladas — o mesmo bug que o `fillDayGaps` (costs.ts:249) conserta no gráfico
  // principal. O teto de 400 passos é guarda contra chave malformada virar laço infinito.
  const xs = [...acc.keys()].sort();
  const passo = semanal ? 7 : 1;
  const out: Ponto[] = [];
  let x = xs[0];
  const fim = xs[xs.length - 1];
  for (let n = 0; n < 400 && x <= fim; n++) {
    out.push({ x, valores: acc.get(x) ?? chaves.map(() => 0) });
    x = somaDias(x, passo);
  }
  return out;
}

const zero = (key: string): DimBucket => ({
  key, label: null, sessions: 0, input: 0, output: 0, cache_write: 0, cache_read: 0,
  cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0,
});

// Um balde por chave, NA ORDEM pedida — a ordem é a das cores dos cartões, e reordenar por custo
// trocaria a cor de uma entidade assim que a outra passasse na frente no meio do período.
// Chave sem nenhuma linha devolve balde zerado em vez de sumir: entidade marcada tem que
// continuar na tela mostrando zero, senão o clique parece não ter funcionado.
export function totaisComparados(
  combos: ComboLocal[], dim: DimFiltro, chaves: string[],
): DimBucket[] {
  const porChave = new Map(agruparPor(combos, dim).map((b) => [b.key, b]));
  return chaves.map((k) => porChave.get(k) ?? zero(k));
}

// Modelos que estão INFLANDO a conta Anthropic no recorte: sem tarifa no catálogo E numa linha do
// Claude Code atribuída a uma conta `anthropic:*`.
//
// O porquê: `costs_sources.linhas_claude` (backend, :100-105) resolve o provedor pela TARIFA do
// modelo (`pricing.provider_for` -> `rate_for`), não pelo id. Modelo que o catálogo não conhece
// devolve None e a linha cai na conta Anthropic — que é exatamente o lado que este painel compara.
// Pi e Codex não entram: os dois carregam o provedor no próprio log.
export function contaInflada(combos: ComboLocal[], semTarifa: string[]): string[] {
  if (!semTarifa.length) return [];
  const suspeitos = new Set(semTarifa);
  const achados = new Set<string>();
  for (const c of combos) {
    if (c.source === 'claude' && c.provider.startsWith('anthropic:') && suspeitos.has(c.model)) {
      achados.add(c.model);
    }
  }
  return [...achados].sort();
}
