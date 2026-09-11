// Costura entre o que JA esta na tela e um historico recem-lido do backend. O Chat carrega em dois
// tempos (cauda primeiro, historico completo em segundo plano), entao duas listas ordenadas viram
// uma — e isso acontece com o SSE entregando eventos no meio do voo. Regra comum as duas funcoes:
// evento que ja esta na tela NUNCA e trocado nem movido; so entra o que falta, do lado certo.
// Consequencia direta: nada que o Chat removeu a mao (a bolha "queued-" aposentada pelo user_msg
// real) volta, porque so acrescentamos ids ausentes — nunca reconstruimos a lista a partir do que
// veio do backend.
import type { ChatEvent } from './types';

export function queuedMessages(events: ChatEvent[], provider?: string | null): ChatEvent[] {
  return events.filter(e => e.kind === 'user_msg' && e.id.startsWith('queued-') && !e.desistiu
    && (provider !== 'codex' || !e.queued_delivered));
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
  const removed = options.removedIds ?? new Set<string>();
  const clean = history.filter(e => !removed.has(e.id));
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
  return merged.filter(e => !removed.has(e.id));
}
