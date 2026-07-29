import { describe, expect, it } from 'vitest';
import type { ChatEvent } from './types';
import { appendTail, hasSeam, prependOlder } from './history';

const ev = (id: string): ChatEvent => ({ kind: 'user_msg', id, text: id });
const ids = (evs: ChatEvent[] | null) => (evs ?? []).map((e) => e.id);

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
