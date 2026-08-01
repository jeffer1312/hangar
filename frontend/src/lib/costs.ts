import type { CostBucket, CostReport, DimBucket, KindBucket, RateInfo } from './types';

// `Partial` de propósito: é o que chega DO FIO. Um servidor da malha em versão antiga responde
// sem os campos novos, e prometer aqui um objeto completo é justamente o que fazia o front
// quebrar em runtime com o `check` verde.
export interface ServerResult {
  report: Partial<CostReport> | null; // null = servidor falhou/offline
  label?: string;                     // rótulo da máquina, pro aviso de "não somei este"
}

export interface MergedReport {
  report: CostReport;
  partial: boolean;      // algum servidor não respondeu ou entrou fora da soma
  mismatched: string[];  // servidores que não ecoaram o período pedido
}

// ── Mescla v2: a malha inteira num relatório só ──────────────────────────────

const zeroBucket = (key: string): DimBucket => ({
  key, sessions: 0, input: 0, output: 0, cache_write: 0, cache_read: 0,
  cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0,
});

// `?? 0` em TODA entrada: servidor da malha em versão antiga não manda os campos novos, e
// `undefined + n` vira NaN — que se espalha e apaga a coluna inteira, inclusive as linhas dos
// servidores que mandaram o dado certo. Mesmo motivo do addModels que já existia aqui.
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
  const mismatched: string[] = [];
  const anterior = zeroBucket('anterior');
  let entraram = 0;      // servidores que de fato entraram na soma
  let comAnterior = 0;   // ...e destes, quantos mandaram a janela anterior
  let partial = false;
  let semCache = 0;
  let equivalente = 0;
  let usdBrl: number | null = null;

  results.forEach((res, i) => {
    const r = res.report;
    if (!r) { partial = true; return; }
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
    semCache += r.custo_sem_cache ?? 0;
    equivalente += r.equivalente_cobrado ?? 0;
    if (r.anterior) { somarBucket(anterior, r.anterior); comAnterior += 1; }
  });

  return {
    partial, mismatched,
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
      applied: { period },
      usd_brl: usdBrl,
    },
  };
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
