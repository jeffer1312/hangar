import { describe, it, expect } from 'vitest';
import { precisaPreencher } from './window';

describe('precisaPreencher', () => {
  it('lista que nao rola e tem historico acima: precisa revelar', () => {
    // O caso real: 120 eventos crus viraram ~20 linhas (rajada de tool calls colapsada em grupo),
    // scrollHeight == clientHeight -> nenhum `onscroll` nunca -> paginacao pra cima nunca dispara.
    expect(precisaPreencher(800, 800, true)).toBe(true);
  });

  it('rolagem menor que a folga de 64px conta como "nao rola"', () => {
    expect(precisaPreencher(840, 800, true)).toBe(true);
    expect(precisaPreencher(880, 800, true)).toBe(false);
  });

  it('sem historico acima nao revela nada (nao ha o que paginar)', () => {
    expect(precisaPreencher(800, 800, false)).toBe(false);
  });
});
