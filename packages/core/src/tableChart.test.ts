import { describe, it, expect, beforeEach } from 'vitest';
import { parseNumero, formatarValor, lerTabelaMarkdown } from './tableChart';
import { overwriteGetLocale } from './paraglide/runtime';

beforeEach(() => overwriteGetLocale(() => 'pt'));

describe('parseNumero', () => {
  it('lê as formas que aparecem numa tabela minha de verdade', () => {
    expect(parseNumero('327')).toBe(327);
    expect(parseNumero('46,9M')).toBeCloseTo(46_900_000);
    expect(parseNumero('5,3M')).toBeCloseTo(5_300_000);
    expect(parseNumero('16k')).toBe(16_000);
  });

  it('resolve pt-BR e en pelo separador que vem por ÚLTIMO', () => {
    expect(parseNumero('1.234,56')).toBeCloseTo(1234.56);
    expect(parseNumero('1,234.56')).toBeCloseTo(1234.56);
    expect(parseNumero('46,9')).toBeCloseTo(46.9);
    expect(parseNumero('1.234.567')).toBe(1_234_567);
  });

  it('um ponto só fica DECIMAL — o caso é ambíguo e chutar milhar erra por 1000x', () => {
    expect(parseNumero('1.234')).toBeCloseTo(1.234);
  });

  it('aceita sinal e porcentagem', () => {
    expect(parseNumero('-3,5%')).toBeCloseTo(-3.5);
    expect(parseNumero('+12')).toBe(12);
  });

  it('devolve null pro que NÃO é número — é isso que separa rótulo de dado', () => {
    expect(parseNumero('Kimi Code')).toBeNull();
    expect(parseNumero('')).toBeNull();
    expect(parseNumero('16k tokens')).toBeNull();
    expect(parseNumero('533 KB')).toBeNull();
    expect(parseNumero('—')).toBeNull();
  });

  it('versão de pacote NÃO é número', () => {
    expect(parseNumero('1.6.32')).toBeNull();
    expect(parseNumero('4.5.1')).toBeNull();
    expect(parseNumero('0.0.7')).toBeNull();
    expect(parseNumero('1.234.567')).toBe(1_234_567);
  });
});

describe('formatarValor', () => {
  it('encurta na magnitude certa', () => {
    expect(formatarValor(46_900_000)).toBe('46,9M');
    expect(formatarValor(16_000)).toBe('16k');
    expect(formatarValor(327)).toBe('327');
  });
});

describe('lerTabelaMarkdown', () => {
  const CUSTO_MD = `|  | chamadas | bruto | cobrável | por chamada |\n|---|---|---|---|---|\n| Kimi Code | 327 | 46,9M | 5,3M | 16k |\n| Pi | 769 | 132,5M | 16,3M | 21k |`;

  it('separa rótulos das colunas numéricas', () => {
    const t = lerTabelaMarkdown(CUSTO_MD);
    expect(t).toHaveLength(1);
    expect(t[0].rotulos).toEqual(['Kimi Code', 'Pi']);
    expect(t[0].colunas.map((c) => c.titulo)).toEqual(['chamadas', 'bruto', 'cobrável', 'por chamada']);
    expect(t[0].colunas[1].valores).toEqual([46_900_000, 132_500_000]);
  });

  it('recusa tabela sem nenhuma coluna numérica → []', () => {
    const md = `| arquivo | o que faz |\n|---|---|\n| a.ts | lê |\n| b.ts | escreve |`;
    expect(lerTabelaMarkdown(md)).toEqual([]);
  });

  it('tabela de uma linha só não é gráfico → []', () => {
    const md = `| a |\n|---|\n| 1 |`;
    expect(lerTabelaMarkdown(md)).toEqual([]);
  });

  it('coluna com UMA célula não-numérica deixa de ser numérica → sem colunas', () => {
    const md = `| x | v |\n|---|---|\n| a | 1 |\n| b | n/a |`;
    expect(lerTabelaMarkdown(md)).toEqual([]);
  });

  it('lê múltiplas tabelas no mesmo texto', () => {
    const md = `${CUSTO_MD}\n\ntexto no meio\n\n| x | v |\n|---|---|\n| a | 1 |\n| b | 2 |`;
    const t = lerTabelaMarkdown(md);
    expect(t).toHaveLength(2);
    expect(t[1].rotulos).toEqual(['a', 'b']);
  });
});
