import { describe, it, expect } from 'vitest';
import { formatClock, cortarAmostra } from './ttsFormat';

describe('formatClock', () => {
  it('formata segundos como m:ss', () => {
    expect(formatClock(0)).toBe('0:00');
    expect(formatClock(9)).toBe('0:09');
    expect(formatClock(75)).toBe('1:15');
    expect(formatClock(600)).toBe('10:00');
  });

  it('nao quebra com valor invalido do elemento de audio', () => {
    // duration de um <audio> ainda sem metadados vem NaN, e Infinity em stream sem tamanho.
    expect(formatClock(NaN)).toBe('0:00');
    expect(formatClock(Infinity)).toBe('0:00');
    expect(formatClock(-3)).toBe('0:00');
  });
});

describe('cortarAmostra', () => {
  it('nao corta texto que ja cabe no limite', () => {
    expect(cortarAmostra('oi', 200)).toBe('oi');
    expect(cortarAmostra('a'.repeat(200), 200)).toBe('a'.repeat(200));
  });

  it('corta na ultima frase inteira que couber', () => {
    const frase1 = 'a'.repeat(50) + '.';
    const frase2 = ' ' + 'b'.repeat(50) + '.';
    const frase3 = ' ' + 'c'.repeat(200);   // ultrapassa o limite de 200
    const texto = frase1 + frase2 + frase3;
    const cortado = cortarAmostra(texto, 200);
    expect(cortado).toBe(frase1 + frase2);
    expect(cortado.endsWith('.')).toBe(true);
    expect(cortado.length).toBeLessThanOrEqual(200);
  });

  it('sem frase completa, corta na ultima palavra inteira', () => {
    const texto = 'a'.repeat(190) + ' ' + 'b'.repeat(50);   // sem ponto nenhum
    const cortado = cortarAmostra(texto, 200);
    expect(cortado).toBe('a'.repeat(190));
    expect(cortado.length).toBeLessThanOrEqual(200);
  });

  it('sem fronteira nenhuma (uma palavra so), cai pro corte duro no limite', () => {
    const texto = 'a'.repeat(300);
    expect(cortarAmostra(texto, 200)).toBe('a'.repeat(200));
  });
});
