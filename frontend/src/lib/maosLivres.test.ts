// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from 'vitest';
import { lerMaosLivres, setMaosLivres } from './maosLivres';

describe('preferência mãos-livres', () => {
  beforeEach(() => { localStorage.clear(); });

  it('vem desligada por padrão', () => {
    expect(lerMaosLivres()).toBe(false);
  });

  it('persiste o que foi escolhido', () => {
    setMaosLivres(true);
    expect(lerMaosLivres()).toBe(true);
    setMaosLivres(false);
    expect(lerMaosLivres()).toBe(false);
  });

  it('não quebra quando o storage está bloqueado (modo privado)', () => {
    // `delete globalThis.localStorage` NAO serve: localStorage e um getter no prototipo de Window, e
    // delete numa propriedade herdada nao remove nada — o teste passaria sem testar. Aqui o getter
    // LANCA, que e exatamente o que o Safari privado faz.
    const orig = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
    Object.defineProperty(globalThis, 'localStorage', {
      configurable: true,
      get() { throw new Error('storage bloqueado'); },
    });
    try {
      expect(() => setMaosLivres(true)).not.toThrow();
      expect(lerMaosLivres()).toBe(false);
    } finally {
      if (orig) Object.defineProperty(globalThis, 'localStorage', orig);
      else Reflect.deleteProperty(globalThis, 'localStorage');
    }
  });
});
