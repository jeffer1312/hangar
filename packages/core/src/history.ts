// Costura entre o que JA esta na tela e um historico recem-lido do backend. O Chat carrega em dois
// tempos (cauda primeiro, historico completo em segundo plano), entao duas listas ordenadas viram
// uma — e isso acontece com o SSE entregando eventos no meio do voo. Regra comum as duas funcoes:
// evento que ja esta na tela NUNCA e trocado nem movido; so entra o que falta, do lado certo.
// Consequencia direta: nada que o Chat removeu a mao (a bolha "queued-" aposentada pelo user_msg
// real) volta, porque so acrescentamos ids ausentes — nunca reconstruimos a lista a partir do que
// veio do backend.
import type { ChatEvent } from './types';
import { donoDaLinha } from './covers';

export function queuedMessages(events: ChatEvent[], provider?: string | null, headless = false): ChatEvent[] {
  // Codex e Claude sem terminal: a fila é do processo, entregue não conta mais como espera.
  const filaDoProcesso = provider === 'codex' || headless;
  return events.filter(e => e.kind === 'user_msg' && e.id.startsWith('queued-') && !e.desistiu
    && (!filaDoProcesso || !e.queued_delivered));
}

/** Historico COMPLETO recem-buscado + a cauda que ja esta na tela -> lista com os eventos
 *  ANTERIORES a cauda prependados. `null` = nada a fazer (o chamador mantem o que tem):
 *  ou ja temos desde o comeco, ou nao ha um evento em comum pra costurar (transcript trocado por
 *  /clear no meio do voo). Tudo que `full` traz DO ponto de costura em diante e ignorado de
 *  proposito: dali pra frente a tela e a verdade, porque o SSE continuou chegando. */
export function prependOlder(full: ChatEvent[], current: ChatEvent[]): ChatEvent[] | null {
  if (!current.length) return full.length ? full : null;
  const have = new Set(current.map((e) => e.id));
  const cut = full.findIndex((e) => have.has(e.id));
  if (cut <= 0) return null;
  return [...full.slice(0, cut), ...current];
}

/** Ha um evento em comum entre o historico recem-lido e o que esta na tela? E o que distingue os
 *  DOIS motivos de `prependOlder` devolver null: sem costura (transcript trocado no meio do voo ->
 *  a conversa fica truncada e o usuario precisa saber) ou "ja temos desde o comeco" (normal, calado).
 *  Lista vazia dos dois lados nao e problema nenhum -> true. */
export function hasSeam(full: ChatEvent[], current: ChatEvent[]): boolean {
  if (!full.length || !current.length) return true;
  const have = new Set(current.map((e) => e.id));
  return full.some((e) => have.has(e.id));
}

/** Cauda recem-lida (volta do segundo plano) -> so o que e NOVO entra no fim; o resto da lista
 *  fica intacto. ASSUME ordem cronologica nos dois lados e que `fresh` e a parte MAIS RECENTE: o
 *  que nao esta em `current` vai pro FIM, sem reordenar (o inverso do corte de prependOlder, que
 *  so aceita o que vem ANTES do ponto de costura). Sem NENHUMA sobreposicao (ficou tempo demais
 *  fora e a cauda pulou o que tinhamos, ou o transcript trocou) a cauda passa a ser a verdade:
 *  melhor uma lista curta e continua do que duas metades sem o meio — o historico antigo volta
 *  logo depois pela carga de fundo. O chamador detecta esse caso comparando o primeiro id
 *  antes/depois. */
export function appendTail(tail: ChatEvent[], current: ChatEvent[]): ChatEvent[] {
  if (!tail.length || !current.length) return tail.length ? tail : current;
  const have = new Set(current.map((e) => e.id));
  const fresh = tail.filter((e) => !have.has(e.id));
  if (fresh.length === tail.length) return tail;
  return fresh.length ? [...current, ...fresh] : current;
}

/** Mensagem real que chegou pelo REST aposenta a bolha da fila dona da linha, como o caminho do SSE
 *  (Chat.svelte). Só reais que não estavam na tela, e só bolhas enfileiradas antes dela; a real cuja
 *  bolha o Chat já aposentou (`retiredBefore`) não leva outra junto. */
function retireQueuedCoveredBy(events: ChatEvent[], fresh: ReadonlySet<string>, retiredBefore: ChatEvent[]):
  { events: ChatEvent[]; retired: string[] } {
  let out = events;
  const retired: string[] = [];
  let pending = retiredBefore.filter((e) => !!e.text);
  for (const real of events) {
    if (real.kind !== 'user_msg' || !real.text || real.id.startsWith('queued-') || !fresh.has(real.id)) continue;
    const consumed = donoDaLinha(real.text, pending.map((e) => e.text!));
    if (consumed >= 0) { pending = pending.filter((_, i) => i !== consumed); continue; }
    const filas = out.flatMap((e, i) => (e.kind === 'user_msg' && e.id.startsWith('queued-') && e.text
      && (real.ts == null || e.queued_ts == null || real.ts >= e.queued_ts) ? [{ i, text: e.text }] : []));
    const dono = donoDaLinha(real.text, filas.map((f) => f.text));
    if (dono < 0) continue;
    retired.push(out[filas[dono].i].id);
    out = [...out.slice(0, filas[dono].i), ...out.slice(filas[dono].i + 1)];
  }
  return { events: out, retired };
}

/** Junta a cauda REST com eventos que o SSE já colocou na tela durante a mesma carga. */
export function mergeHistoryWithLive(
  history: ChatEvent[],
  current: ChatEvent[],
  options: {
    preserveNoSeam?: boolean;
    removedIds?: ReadonlySet<string>;
    cachedEvents?: ReadonlySet<ChatEvent>;
  } = {},
): ChatEvent[] {
  return mergeHistoryWithLiveRetiring(history, current, options).events;
}

/** Igual ao `mergeHistoryWithLive`, devolvendo também as bolhas da fila aposentadas: quem chama as
 *  soma ao que já retirou, para a reconexão do SSE não devolvê-las. */
export function mergeHistoryWithLiveRetiring(
  history: ChatEvent[],
  current: ChatEvent[],
  options: {
    preserveNoSeam?: boolean;
    removedIds?: ReadonlySet<string>;
    cachedEvents?: ReadonlySet<ChatEvent>;
  } = {},
): { events: ChatEvent[]; retired: string[] } {
  const removed = options.removedIds ?? new Set<string>();
  const retiredBefore = current.filter(e => removed.has(e.id) && e.id.startsWith('queued-'));
  const clean = history.filter(e => !removed.has(e.id));
  const onScreen = new Set(current.map(e => e.id));
  const fresh = new Set(clean.filter(e => !onScreen.has(e.id)).map(e => e.id));
  current = current.filter(e => !removed.has(e.id));
  let merged: ChatEvent[];
  if (!hasSeam(clean, current)) {
    const live = options.preserveNoSeam
      ? current
      : options.cachedEvents
        ? current.filter((e) => !options.cachedEvents!.has(e))
        : [];
    merged = [...clean, ...live];
  } else {
    const historyIds = new Set(clean.map(e => e.id));
    const positions = new Map(current.map((e, i) => [e.id, i]));
    const first = current.findIndex(e => historyIds.has(e.id));
    // A fila também chega por SSE; um eco novo não é parte do histórico anterior à cauda.
    merged = current.slice(0, Math.max(first, 0))
      .filter(e => !e.id.startsWith('queued-') || options.cachedEvents?.has(e));
    const precedingIds = new Set(merged.map(e => e.id));
    let cursor = 0;
    for (const event of clean) {
      const position = positions.get(event.id);
      if (position !== undefined) {
        while (cursor < position) {
          const preceding = current[cursor++];
          if (!historyIds.has(preceding.id) && !precedingIds.has(preceding.id)) merged.push(preceding);
        }
        cursor = Math.max(cursor, position + 1);
      }
      merged.push(position === undefined ? event : current[position]);
    }
    merged.push(...current.slice(cursor).filter(e => !historyIds.has(e.id)));
  }
  return retireQueuedCoveredBy(merged.filter(e => !removed.has(e.id)), fresh, retiredBefore);
}
