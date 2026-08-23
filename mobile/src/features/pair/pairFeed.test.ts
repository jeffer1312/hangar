import { describe, expect, it } from 'vitest';
import type { ChatEvent } from '@hangar/core';
import { montarFeed } from './pairFeed';

function user(id: string, text: string, ts: number): ChatEvent {
  return { kind: 'user_msg', id, text, ts };
}

describe('montarFeed', () => {
  it('filtra recados de outros membros, ordena e mantém só os 40 mais recentes', () => {
    const historyA = Array.from({ length: 42 }, (_, i) => user(`a-${i}`, `[de: b] recado ${i}`, i + 1));
    historyA.push(user('self', '[de: a] não é recado de outro membro', 100));
    historyA.push({ kind: 'assistant_msg', id: 'assistant', text: '[de: b] não é user_msg', ts: 101 });

    const result = montarFeed(
      ['a', 'b'],
      [
        { ok: true, h: historyA },
        { ok: true, h: [user('b-1', 'mensagem normal', 2)] },
      ],
    );

    expect(result.failed).toEqual([]);
    expect(result.feed).toHaveLength(40);
    expect(result.feed[0]).toEqual({ from: 'b', to: 'a', text: 'recado 2', ts: 3 });
    expect(result.feed.at(-1)).toEqual({ from: 'b', to: 'a', text: 'recado 41', ts: 42 });
  });

  it('devolve os membros cujo histórico falhou sem tratar falha como conversa vazia', () => {
    const result = montarFeed(
      ['a', 'b', 'c'],
      [
        { ok: false, h: [] },
        { ok: true, h: [user('b-1', '[grupo: c] chegou', 20)] },
        { ok: false, h: [] },
      ],
    );

    expect(result.failed).toEqual(['a', 'c']);
    expect(result.feed).toEqual([{ from: 'c', to: 'b', text: 'chegou', ts: 20 }]);
  });
});
