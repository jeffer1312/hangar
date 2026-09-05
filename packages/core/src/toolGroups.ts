// Regra de montagem da lista do chat, compartilhada pelas duas telas (PWA e app nativo): o que vira
// bolha, o que vira card de ferramenta, o que colapsa em grupo e o que some dentro do pensamento.
// Mora aqui porque as duas views precisam da MESMA regra — a versao duplicada ja tinha divergido.
import type { ChatEvent } from './types';
import { chavesUnicas } from './messageKeys';

export type ItemConversa =
  | { type: 'event'; id: string; ev: ChatEvent }
  | { type: 'tool'; id: string; ev: ChatEvent }
  | { type: 'group'; id: string; tools: ChatEvent[] }
  | { type: 'pensamento'; id: string; eventos: ChatEvent[] };

export interface OpcoesAgrupar {
  /** Esta chamada, feita no meio do raciocinio, some dentro do bloco de pensamento? */
  entraNoPensamento: (nome?: string | null) => boolean;
  /** A partir de quantas chamadas seguidas vira grupo. 1-2 ficam soltas: nao e bagunca. */
  groupMin?: number;
}

export function agruparConversa(eventos: ChatEvent[], opts: OpcoesAgrupar): ItemConversa[] {
  const groupMin = opts.groupMin ?? 3;
  const items: ItemConversa[] = [];
  let run: ChatEvent[] = [];
  const flush = () => {
    // Key do grupo = 1o tool id: estavel enquanto o run cresce na cauda.
    if (run.length >= groupMin) items.push({ type: 'group', id: `g-${run[0].id}`, tools: run });
    else for (const t of run) items.push({ type: 'tool', id: t.id, ev: t });
    run = [];
  };
  // Pensamentos consecutivos (e as buscas entre eles) viram UM bloco recolhido. Qualquer outra
  // ferramenta fecha o bloco: ela e trabalho visivel, e o pensamento seguinte abre outro bloco.
  let pens: ChatEvent[] = [];
  const flushPens = () => {
    if (pens.length) items.push({ type: 'pensamento', id: `p-${pens[0].id}`, eventos: pens });
    pens = [];
  };
  for (const ev of eventos) {
    // tool_result nunca e item proprio: ele entra no card do tool_use pareado.
    if (ev.kind === 'tool_result') continue;
    if (ev.kind === 'thinking') { flush(); pens.push(ev); continue; }
    // Busca so e engolida quando ha um pensamento ABERTO antes dela — busca solta continua card
    // normal, senao sumiria numa linha que nao explica nada.
    if (pens.length && ev.kind === 'tool_use' && opts.entraNoPensamento(ev.tool_name)) { pens.push(ev); continue; }
    flushPens();
    if (ev.kind === 'tool_use') { run.push(ev); continue; }
    flush();
    items.push({ type: 'event', id: ev.id, ev });
  }
  flush();
  flushPens();
  // Rede de seguranca da lista keyed: chave repetida e throw no Svelte e item duplicado na FlatList.
  const chaves = chavesUnicas(items.map((i) => i.id));
  return items.map((i, n) => (chaves[n] === i.id ? i : { ...i, id: chaves[n] }));
}
