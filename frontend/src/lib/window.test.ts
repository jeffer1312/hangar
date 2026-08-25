import { describe, it, expect } from 'vitest';
import { precisaPreencher, mostrarIrPraoFim } from './window';

describe('mostrarIrPraoFim', () => {
  it('a faixa morta: janela congelada com evento novo e ainda sem uma tela rolada', () => {
    // O caso relatado em 25/08/2026. Rolou pouco (scrolledUp falso, porque nao passou de uma tela)
    // mas o suficiente pra sair dos 64px do atBottom -> a janela congelou em 40 com 45 eventos.
    // Antes disto o botao ficava escondido e o chat parava calado.
    expect(mostrarIrPraoFim(false, 40, 45)).toBe(true);
  });

  it('rolou mais de uma tela: continua aparecendo mesmo sem evento novo', () => {
    expect(mostrarIrPraoFim(true, 45, 45)).toBe(true);
  });

  it('colado no fim e em dia: nao aparece', () => {
    expect(mostrarIrPraoFim(false, 45, 45)).toBe(false);
    expect(mostrarIrPraoFim(false, 0, 0)).toBe(false);
  });
});

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
