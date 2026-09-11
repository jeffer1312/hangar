import { describe, expect, it } from 'vitest';
import type { ChatEvent } from './types';
import { appendTail, hasSeam, mergeHistoryWithLive, prependOlder, queuedMessages } from './history';

const ev = (id: string): ChatEvent => ({ kind: 'user_msg', id, text: id });
const ids = (evs: ChatEvent[] | null) => (evs ?? []).map((e) => e.id);

it('Codex conta só mensagens ainda não encaminhadas, preservando Kimi e o legado', () => {
  const events: ChatEvent[] = [ev('queued-legado'), { ...ev('queued-enviada'), queued_delivered: true },
    { ...ev('queued-pendente'), queued_delivered: false }, { ...ev('queued-perdida'), desistiu: true },
    ev('real'), { ...ev('queued-task'), kind: 'tool_result' }];
  expect(ids(queuedMessages(events, 'codex'))).toEqual(['queued-legado', 'queued-pendente']);
  expect(ids(queuedMessages(events, 'kimi'))).toEqual(['queued-legado', 'queued-enviada', 'queued-pendente']);
  expect(events).toHaveLength(6);
});

describe('prependOlder', () => {
  it('traz so o que e mais antigo que a cauda em tela', () => {
    const full = ['a', 'b', 'c', 'd'].map(ev);
    const cur = ['c', 'd'].map(ev);
    expect(ids(prependOlder(full, cur))).toEqual(['a', 'b', 'c', 'd']);
  });

  it('nao sobrescreve o que chegou pelo SSE no meio-tempo', () => {
    const full = ['a', 'b'].map(ev);          // foto tirada antes do SSE entregar 'c'
    const cur = [ev('b'), ev('c')];
    const out = prependOlder(full, cur)!;
    expect(ids(out)).toEqual(['a', 'b', 'c']);
    expect(out[1]).toBe(cur[0]);               // objeto da tela preservado, nao o do backend
  });

  it('nao ressuscita evento removido a mao (dedup queued-)', () => {
    const full = ['a', 'b', 'c'].map(ev);
    const cur = ['a', 'c'].map(ev);            // 'b' foi removido pelo dedup do Chat
    expect(prependOlder(full, cur)).toBeNull(); // ja temos desde o comeco -> nada a fazer
    expect(ids(prependOlder(['z', ...['a', 'b', 'c']].map(ev), cur))).toEqual(['z', 'a', 'c']);
  });

  it('devolve null sem ponto de costura (transcript trocado) e com cauda vazia devolve o full', () => {
    expect(prependOlder(['x', 'y'].map(ev), ['a'].map(ev))).toBeNull();
    expect(ids(prependOlder(['x'].map(ev), []))).toEqual(['x']);
    expect(prependOlder([], [])).toBeNull();
  });
});

describe('hasSeam', () => {
  // Separa os DOIS nulls do prependOlder: só o sem-costura vira aviso na tela.
  it('so acusa quando nao ha id em comum (transcript trocado)', () => {
    const cur = ['c', 'd'].map(ev);
    expect(hasSeam(['a', 'b', 'c', 'd'].map(ev), cur)).toBe(true);   // da pra costurar
    expect(hasSeam(['c', 'd'].map(ev), cur)).toBe(true);             // ja temos desde o comeco
    expect(hasSeam(['x', 'y'].map(ev), cur)).toBe(false);            // outro transcript
    expect(hasSeam([], cur)).toBe(true);                             // nada veio: nao e buraco
    expect(hasSeam(['a'].map(ev), [])).toBe(true);                   // tela vazia: prependOlder resolve
  });
});

describe('appendTail', () => {
  it('acrescenta so o que e novo, sem mexer no resto', () => {
    const cur = ['a', 'b', 'c'].map(ev);
    const out = appendTail(['b', 'c', 'd'].map(ev), cur);
    expect(ids(out)).toEqual(['a', 'b', 'c', 'd']);
    expect(out[0]).toBe(cur[0]);
  });

  it('sem sobreposicao (buraco) a cauda vira a verdade', () => {
    const out = appendTail(['x', 'y'].map(ev), ['a', 'b'].map(ev));
    expect(ids(out)).toEqual(['x', 'y']);
  });

  it('cauda vazia ou ja contida nao muda a lista', () => {
    const cur = ['a', 'b'].map(ev);
    expect(appendTail([], cur)).toBe(cur);
    expect(appendTail(['a', 'b'].map(ev), cur)).toBe(cur);
    expect(ids(appendTail(['a'].map(ev), []))).toEqual(['a']);
  });
});

describe('mergeHistoryWithLive', () => {
  it('insere mensagens faltantes entre o cache e o SSE', () => {
    expect(ids(mergeHistoryWithLive(['a', 'b', 'c'].map(ev), ['a', 'c', 'd'].map(ev))))
      .toEqual(['a', 'b', 'c', 'd']);
    expect(ids(mergeHistoryWithLive(['a', 'b'].map(ev), ['a', 'c'].map(ev))))
      .toEqual(['a', 'b', 'c']);
  });

  it('mantém o histórico antigo antes da cauda REST quando só o fim é comum', () => {
    expect(ids(mergeHistoryWithLive(['b', 'c'].map(ev), ['a', 'c'].map(ev))))
      .toEqual(['a', 'b', 'c']);
    expect(ids(mergeHistoryWithLive(['c', 'd'].map(ev), ['a', 'b', 'd'].map(ev))))
      .toEqual(['a', 'b', 'c', 'd']);
  });

  it('eco recebido durante a carga fica depois do prefixo histórico', () => {
    expect(ids(mergeHistoryWithLive(['a', 'b', 'c'].map(ev), ['queued-q', 'c'].map(ev), {
      cachedEvents: new Set(),
    }))).toEqual(['a', 'b', 'queued-q', 'c']);
  });

  it('retira ecos confirmados tanto do cache quanto do REST', () => {
    const removedIds = new Set(['queued-q']);
    expect(ids(mergeHistoryWithLive(['a', 'queued-q', 'b'].map(ev), ['a', 'queued-q', 'c'].map(ev), { removedIds })))
      .toEqual(['a', 'b', 'c']);
  });

  it('retira eco confirmado do cache preservando a mensagem real', () => {
    const q = { ...ev('queued-q'), text: 'pode continuar agr' };
    const real = { ...ev('real'), text: q.text };
    const removedIds = new Set([q.id]);
    expect(ids(mergeHistoryWithLive([ev('a'), real, ev('b')], [ev('a'), q], { removedIds })))
      .toEqual(['a', 'real', 'b']);
  });

  it('uma mensagem real não apaga duas entradas iguais da fila', () => {
    const q = { ...ev('queued-q'), text: 'ok' };
    const other = { ...q, id: 'queued-other' };
    const real = { ...ev('real'), text: 'ok' };
    expect(ids(mergeHistoryWithLive([ev('a'), real], [ev('a'), q, other], { removedIds: new Set([q.id]) })))
      .toEqual(['a', 'real', 'queued-other']);
  });

  it('insere o prefixo antigo antes da cauda viva', () => {
    expect(ids(mergeHistoryWithLive(['a', 'b', 'c'].map(ev), ['c'].map(ev))))
      .toEqual(['a', 'b', 'c']);
  });

  it('acrescenta o sufixo novo que veio no REST', () => {
    expect(ids(mergeHistoryWithLive(['a', 'b', 'c'].map(ev), ['a', 'b'].map(ev))))
      .toEqual(['a', 'b', 'c']);
  });

  it('mantem eventos ao vivo posteriores quando ainda nao ha ponto comum', () => {
    expect(ids(mergeHistoryWithLive(
      ['a', 'b'].map(ev), ['c'].map(ev), { preserveNoSeam: true },
    )))
      .toEqual(['a', 'b', 'c']);
  });

  it('sem ponto comum descarta cache de outro transcript por padrão', () => {
    expect(ids(mergeHistoryWithLive(['novo'].map(ev), ['antigo'].map(ev))))
      .toEqual(['novo']);
  });

  it('preserva a posição de fila já visível', () => {
    expect(ids(mergeHistoryWithLive(['a', 'b'].map(ev), ['a', 'queued-q', 'b'].map(ev))))
      .toEqual(['a', 'queued-q', 'b']);
  });

  it('não ressuscita fila removida enquanto o REST estava em voo', () => {
    expect(ids(mergeHistoryWithLive(
      ['a', 'queued-q'].map(ev), ['a'].map(ev), { removedIds: new Set(['queued-q']) },
    ))).toEqual(['a']);
  });

  it('descarta só o cache incompatível e preserva evento que chegou durante o REST', () => {
    const cached = ev('antigo');
    const live = ev('queued-atual');
    expect(ids(mergeHistoryWithLive(
      ['novo'].map(ev), [cached, live], { cachedEvents: new Set([cached]) },
    ))).toEqual(['novo', 'queued-atual']);
  });
});
