import { describe, expect, it } from 'vitest';
import { dec, money, money2, tok } from './fmt';

describe('formatação pt-BR', () => {
  it('token grande usa vírgula decimal, não ponto', () => {
    // "19.83 Bi" numa tela que também mostra "US$ 17.730,43" lê como dezenove MIL:
    // o ponto muda de significado no meio da página.
    expect(tok(19_830_000_000)).toBe('19,83 Bi');
    expect(tok(489_600_000)).toBe('489,6 Mi');
    expect(tok(12)).toBe('12');
  });

  it('a fronteira de escala não produz "1.000 mil"', () => {
    // 999.999 com 0 casas arredonda pra "1.000" e o sufixo "mil" tornaria o número ilegível —
    // exatamente a ambiguidade que este módulo existe pra matar. Sobe de escala.
    expect(tok(999_999)).toBe('1,0 Mi');
    expect(tok(999_999_999)).toBe('1,00 Bi');
    // e não sobe cedo demais:
    expect(tok(999_400)).toBe('999 mil');
    expect(tok(999_900_000)).toBe('999,9 Mi');
  });

  it('money compacta acima de mil e money2 nunca', () => {
    expect(money(105_336.79, 'USD', null)).toBe('US$ 105,3 mil');
    expect(money2(105_336.79, 'USD', null)).toBe('US$ 105.336,79');
    expect(money(12.5, 'USD', null)).toBe('US$ 12,50');
  });

  it('BRL converte pela cotação e USD ignora', () => {
    expect(money2(10, 'BRL', 5)).toBe('R$ 50,00');
    expect(money2(10, 'USD', 5)).toBe('US$ 10,00');
  });

  it('sem cotação, BRL cai em USD em vez de mostrar número errado', () => {
    expect(money2(10, 'BRL', null)).toBe('US$ 10,00');
  });

  it('dec respeita as casas pedidas', () => {
    expect(dec(5.0746, 2)).toBe('5,07');
  });
});
