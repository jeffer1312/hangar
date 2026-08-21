import type {
  ComboLocal, ComboRow, CostBucket, CostReport, DimBucket, KindBucket, RateInfo,
} from './types';

// `Partial` de propósito: é o que chega DO FIO. Um servidor da malha em versão antiga responde
// sem os campos novos, e prometer aqui um objeto completo é justamente o que fazia o front
// quebrar em runtime com o `check` verde.
export interface ServerResult {
  report: Partial<CostReport> | null; // null = servidor falhou/offline
  label?: string;                     // rótulo da máquina, pro aviso de "não somei este"
  // Id estável da máquina (`Server.id` do lib/auth). É a CHAVE do corte por servidor: o rótulo é
  // editável e pode repetir entre duas máquinas, e duas "Casa" viravam um balde só.
  id?: string;
}

// O relatório mesclado NÃO é um CostReport: ele tem duas coisas que nenhum servidor manda — os
// combos carimbados com a máquina e o corte por máquina, que só existem depois de juntar. E aqui
// `combos` é obrigatório (lista vazia quando ninguém mandou), porque a tolerância a servidor
// antigo mora na ENTRADA, não na saída — mesma razão do comentário em types.ts sobre CostReport.
export interface RelatorioMesclado extends Omit<CostReport, 'combos'> {
  combos: ComboLocal[];
  by_servidor: DimBucket[];
}

export interface MergedReport {
  report: RelatorioMesclado;
  partial: boolean;      // algum servidor não respondeu ou entrou fora da soma
  mismatched: string[];  // servidores que não ecoaram o período pedido
  // Servidores que não responderam (offline, timeout, erro). Lista separada do `mismatched`
  // porque as duas causas são diferentes e o aviso da tela precisa dizer QUAL máquina caiu em
  // qual: com as duas acontecendo juntas, só o `mismatched` era nomeado e a máquina caída
  // desaparecia atrás de um "algum servidor não respondeu" que não nomeava ninguém.
  failed: string[];
}

// ── Mescla v2: a malha inteira num relatório só ──────────────────────────────

const zeroBucket = (key: string): DimBucket => ({
  key, label: null, sessions: 0, input: 0, output: 0, cache_write: 0, cache_read: 0,
  cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0,
});

// `?? 0` em TODA entrada: servidor da malha em versão antiga não manda os campos novos, e
// `undefined + n` vira NaN — que se espalha e apaga a coluna inteira, inclusive as linhas dos
// servidores que mandaram o dado certo.
function somarBucket(alvo: DimBucket, b: Partial<DimBucket>): void {
  alvo.sessions += b.sessions ?? 0;
  alvo.input += b.input ?? 0;
  alvo.output += b.output ?? 0;
  alvo.cache_write += b.cache_write ?? 0;
  alvo.cache_read += b.cache_read ?? 0;
  alvo.cost += b.cost ?? 0;
  alvo.cost_input += b.cost_input ?? 0;
  alvo.cost_output += b.cost_output ?? 0;
  alvo.cost_cache_write += b.cost_cache_write ?? 0;
  alvo.cost_cache_read += b.cost_cache_read ?? 0;
}

function juntarDim(destino: Map<string, DimBucket>, lista: DimBucket[] | undefined): void {
  for (const b of lista ?? []) {
    if (!b || typeof b.key !== 'string') continue;
    let alvo = destino.get(b.key);
    if (!alvo) { alvo = zeroBucket(b.key); destino.set(b.key, alvo); }
    // O rótulo é do PRIMEIRO servidor que souber dizer: a chave é a mesma nos dois (é o que
    // permite somar), e um servidor da malha em versão antiga manda a linha sem `label` — sem
    // este `??`, a máquina antiga apagaria o nome que a nova já tinha resolvido.
    alvo.label = alvo.label ?? b.label ?? null;
    somarBucket(alvo, b);
  }
}

const ordenar = (m: Map<string, DimBucket>) =>
  [...m.values()].sort((a, b) => b.cost - a.cost || a.key.localeCompare(b.key));

export function mergeReports(results: ServerResult[], period: string): MergedReport {
  const totals = zeroBucket('totals');
  const dims = {
    by_day: new Map<string, DimBucket>(), by_provider: new Map<string, DimBucket>(),
    by_source: new Map<string, DimBucket>(), by_project: new Map<string, DimBucket>(),
    by_model: new Map<string, DimBucket>(),
  };
  const kinds = new Map<string, KindBucket>();
  const rates = new Map<string, RateInfo>();
  const semTarifa = new Set<string>();
  // O detalhamento cruzado só se CONCATENA: cada linha já é uma combinação daquela máquina, e as
  // dimensões que somam entre servidores (dia, fonte, projeto, modelo) somam depois, no cliente,
  // quando o recorte pedir. Servidor recusado não contribui combo, como não contribui total.
  const combos: ComboLocal[] = [];
  const servidores: DimBucket[] = [];
  const mismatched: string[] = [];
  const failed: string[] = [];
  const anterior = zeroBucket('anterior');
  let entraram = 0;      // servidores que de fato entraram na soma
  let comAnterior = 0;   // ...e destes, quantos mandaram a janela anterior
  let partial = false;
  let semCache = 0;
  let equivalente = 0;
  let usdBrl: number | null = null;

  results.forEach((res, i) => {
    const r = res.report;
    // Mesmo rótulo-em-vez-de-índice do `mismatched` logo abaixo, pelo mesmo motivo.
    if (!r) { partial = true; failed.push(res.label ?? `#${i + 1}`); return; }
    // A cotação é lida ANTES da recusa: USD/BRL não depende de período nenhum, e se a única
    // máquina que tem cotação for a desatualizada, a malha inteira perderia o R$ à toa.
    usdBrl = usdBrl ?? r.usd_brl ?? null;
    // FastAPI ignora query param desconhecido: um backend antigo recebe ?period=7d e devolve
    // TUDO. Somar isso com o recorte dos outros e chamar de "7 dias" é mentira — então ele
    // entra como parcial DECLARADO, fora da soma. O rótulo sai daqui porque `#2` é acoplamento
    // posicional: basta o chamador filtrar a lista antes e o índice passa a apontar pra máquina
    // errada, com o tipo `string[]` intacto e ninguém percebendo.
    if ((r.applied?.period ?? null) !== period) {
      partial = true;
      mismatched.push(res.label ?? `#${i + 1}`);
      return;
    }
    entraram += 1;
    somarBucket(totals, r.totals ?? {});
    // Chave = id; rótulo = nome da máquina. O `??` mantém o comportamento do chamador antigo,
    // que só tinha label.
    const sid = res.id ?? res.label ?? `#${i + 1}`;
    const bs = zeroBucket(sid);
    bs.label = res.label ?? null;
    somarBucket(bs, r.totals ?? {});
    servidores.push(bs);
    juntarDim(dims.by_day, r.by_day);
    juntarDim(dims.by_provider, r.by_provider);
    juntarDim(dims.by_source, r.by_source);
    juntarDim(dims.by_project, r.by_project);
    juntarDim(dims.by_model, r.by_model);
    for (const k of r.by_kind ?? []) {
      const cur = kinds.get(k.kind) ?? { kind: k.kind, tokens: 0, cost: 0 };
      cur.tokens += k.tokens ?? 0;
      cur.cost += k.cost ?? 0;
      kinds.set(k.kind, cur);
    }
    // Chave provider+model: dois provedores podem publicar o MESMO nome de modelo com tarifas
    // diferentes, e chavear só por `model` fundia os dois — vencia o último servidor da lista,
    // calado.
    for (const t of r.rates ?? []) rates.set(`${t.provider}|${t.model}`, t);
    for (const m of r.sem_tarifa ?? []) semTarifa.add(m);
    for (const cb of r.combos ?? []) combos.push({ ...cb, servidor: sid });
    semCache += r.custo_sem_cache ?? 0;
    equivalente += r.equivalente_cobrado ?? 0;
    if (r.anterior) { somarBucket(anterior, r.anterior); comAnterior += 1; }
  });

  return {
    partial, mismatched, failed,
    report: {
      totals,
      by_day: [...dims.by_day.values()].sort((a, b) => b.key.localeCompare(a.key)),
      by_provider: ordenar(dims.by_provider),
      by_source: ordenar(dims.by_source),
      by_project: ordenar(dims.by_project),
      by_model: ordenar(dims.by_model),
      by_kind: [...kinds.values()],
      rates: [...rates.values()].sort((a, b) => a.model.localeCompare(b.model)),
      sem_tarifa: [...semTarifa].sort(),
      custo_sem_cache: semCache,
      equivalente_cobrado: equivalente,
      // Ou TODOS os servidores somados mandaram a janela anterior, ou não há comparação. Um
      // `anterior` parcial é pior que nenhum: `totals` traz as duas máquinas e `anterior` só
      // uma, então a tela mostraria uma alta que não existe. "Sem período anterior completo
      // pra comparar" é a mensagem honesta.
      anterior: entraram > 0 && comAnterior === entraram ? anterior : null,
      combos,
      by_servidor: [...servidores].sort((a, b) => b.cost - a.cost || a.key.localeCompare(b.key)),
      applied: { period },
      usd_brl: usdBrl,
    },
  };
}

// Tarifas indexadas por MODELO, que é como a tela pergunta ("qual o preço de claude-sonnet-4?"),
// enquanto o fio guarda por `provider|model`, que é como o preço existe. Desfazer essa chave
// escolhendo o primeiro provedor é o que mostrava a tarifa da Kimi ao lado de um custo que soma
// Kimi + Z.ai. O Map responde DUAS coisas, e a diferença é o ponto:
//   `has(modelo)`  — existe preço conhecido? Se NÃO, o `cost` que o backend mandou é 0 porque ele
//                    pulou a conta (`costs.py` `_custo_da_linha` devolve None), não porque foi de
//                    graça. Mostrar US$ 0,00 aí afirma "não custou nada", que é uma mentira
//                    diferente de "não sei o preço" — e é a cara do bug.
//   `get(modelo)`  — QUAL tarifa exibir; `null` quando há mais de uma, porque aí não existe uma
//                    tarifa só que descreva aquela linha.
export function tarifasPorModelo(rates: RateInfo[]): Map<string, RateInfo | null> {
  const porModelo = new Map<string, RateInfo[]>();
  for (const t of rates) {
    const l = porModelo.get(t.model);
    if (l) l.push(t); else porModelo.set(t.model, [t]);
  }
  return new Map([...porModelo].map(([model, l]) => [model, l.length === 1 ? l[0] : null]));
}

// "Custo desconhecido", pra QUALQUER corte — provedor, fonte, projeto, modelo ou dia. Volume
// positivo com custo zero só acontece quando não havia tarifa pra aplicar: o catálogo descarta
// entrada de preço 0/0 (`app/pricing.py`), então nenhum token com preço conhecido soma 0.
// A pergunta é a mesma do `tarifasPorModelo`, mas aqui feita ao BALDE, e é assim que ela vale
// fora do eixo de modelo — um provedor cujos modelos sejam todos desconhecidos vinha mostrando
// US$ 0,00 como se fosse gasto real.
export function custoDesconhecido(b: DimBucket): boolean {
  return b.input + b.output + b.cache_write + b.cache_read > 0 && b.cost === 0;
}

// Tier GRÁTIS pelo id: `free`, `-free`, `:free` ou `free:thinking` (o Pi gruda o nível de
// thinking DEPOIS do `:free` — `kimi-k3-free:high` — e a regex que exigia fim-de-string
// classificava essa variante como "sem tarifa", a mentira exata que esta função existe pra
// evitar). Não é "sem tarifa" (preço desconhecido) — é preço zero de verdade. Só muda o RÓTULO:
// o custo continua traço, porque o zero que o backend manda vem de ele ter pulado a conta (a
// tarifa dele nem existe no catálogo), não de uma tarifa de 0.
export function isFree(model: string): boolean {
  return /(^|[\/:-])free(?:$|:)/i.test(model);
}

// Preço PARCIAL: um servidor da malha conhece a tarifa deste modelo e outro (snapshot mais velho
// do catálogo) não. O balde mesclado sai com o volume dos DOIS e o custo de UM: `tarifasPorModelo`
// responde `has() === true`, `custoDesconhecido()` responde false, e a linha mostra um preço
// subestimado sem marca nenhuma. Dentro de um servidor só isso é impossível — é bug exclusivo da
// mescla. O `sem_tarifa` que o servidor atrasado mandou já NOMEIA o modelo; ele só era lido no
// rodapé, e é essa a informação que faltava chegar na linha.
export function precoParcial(model: string, temTarifa: boolean, semTarifa: string[]): boolean {
  return temTarifa && semTarifa.includes(model);
}

// Custo "se nenhum token fosse cache" e o equivalente-input de um RECORTE, calculados no
// cliente. O servidor manda os dois como escalar do período inteiro (`custo_sem_cache` /
// `equivalente_cobrado`); dentro de um filtro esses escalares não existem, mas cada combo
// carrega os quatro tipos de token e a tarifa viaja nos `rates` — a mesma aritmética do
// costs.py, linha a linha. Regra do backend que o cliente espelha: linha sem tarifa não entra
// (pular é "não sei o preço", não "foi de graça").
// ponytail: tarifa por MODELO, não por provider|model — o provider da LINHA é o gateway
// ('opencode-go' servindo deepseek), o do RateInfo é o do catálogo; as chaves nunca casariam.
// O preço por modelo é o que o backend usa (rate_for(model)). Divergência teórica: o mesmo nome
// em 2 provedores da malha devolve null (tarifasPorModelo) e a linha sai do recálculo — raro,
// e o custo dela ainda está no foco; aceito como o traço conservador da tela.
export function custoSemCacheDe(combos: ComboRow[], tarifas: Map<string, RateInfo | null>): number {
  let soma = 0;
  for (const c of combos) {
    const t = tarifas.get(c.model);
    if (!t) continue;
    soma += ((c.input + c.cache_write + c.cache_read) / 1e6 * t.input + c.output / 1e6 * t.output);
  }
  return soma;
}

export function equivalenteDe(combos: ComboRow[], tarifas: Map<string, RateInfo | null>): number {
  let soma = 0;
  for (const c of combos) {
    const t = tarifas.get(c.model);
    if (!t || !t.input) continue;
    soma += c.input + c.output * (t.output / t.input)
      + c.cache_write * (t.cache_write / t.input) + c.cache_read * (t.cache_read / t.input);
  }
  return soma;
}

// Recorte de "esconder da lista" — e a razão de ele morar AQUI e não dentro do .svelte é que a
// garantia que ele tem que dar ("esconder não muda total nenhum") é justamente a candidata
// número um a *o total não bate*, e dentro do componente ela era intestável.
// `pico` sai dos VISÍVEIS: manter a régua num projeto escondido deixaria todas as barras da tela
// curtas por causa de algo que ninguém vê.
export function partirOcultos(lista: DimBucket[], ocultos: Set<string>): {
  visiveis: DimBucket[]; escondidos: DimBucket[]; pico: number;
} {
  const visiveis: DimBucket[] = [];
  const escondidos: DimBucket[] = [];
  for (const b of lista) (ocultos.has(b.key) ? escondidos : visiveis).push(b);
  return { visiveis, escondidos, pico: Math.max(1, ...visiveis.map((b) => b.cost)) };
}

// Preenche buracos de data na lista de buckets diários (desc) com dias zerados, pra série
// visual ficar contínua. Só faz sentido no período "dia" — semana/mês ficam como estão.
export function fillDayGaps(list: CostBucket[]): CostBucket[] {
  if (list.length < 2) return list;
  const zero = (key: string): CostBucket => ({
    key, sessions: 0, input: 0, output: 0, cache_read: 0, cache_write: 0, cost: 0,
  });
  const fmt = (d: Date) =>
    `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  const out: CostBucket[] = [];
  for (let i = 0; i < list.length; i++) {
    out.push(list[i]);
    const next = list[i + 1];
    if (!next) break;
    const d = new Date(`${list[i].key}T00:00:00`);
    // guarda de sanidade: key malformada não pode virar loop infinito
    for (let g = 0; g < 366; g++) {
      d.setDate(d.getDate() - 1);
      const k = fmt(d);
      if (k <= next.key) break;
      out.push(zero(k));
    }
  }
  return out;
}
