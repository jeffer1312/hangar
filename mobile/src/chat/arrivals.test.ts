import { describe, expect, it } from 'vitest';
import { arrivedCount } from './arrivals';

describe('arrivedCount', () => {
  it('conta só o acréscimo pequeno no fim de uma conversa já aberta', () => {
    expect(arrivedCount(['a', 'b'], ['a', 'b', 'c'])).toBe(1);
    expect(arrivedCount(['a', 'b'], ['a', 'b', 'c', 'd', 'e', 'f'])).toBe(4);
  });

  it('conversa abrindo, página antiga, troca da última linha e lote grande aparecem direto', () => {
    expect(arrivedCount(null, ['a'])).toBe(0);
    expect(arrivedCount([], ['a'])).toBe(0);
    expect(arrivedCount(['b', 'c'], ['a', 'b', 'c'])).toBe(0);
    expect(arrivedCount(['a', 'queued-1'], ['a', 'real-1'])).toBe(0);
    expect(arrivedCount(['a'], ['a', 'b', 'c', 'd', 'e', 'f'])).toBe(0);
  });
});
