// @vitest-environment happy-dom
// `lerTabela` recebe uma <table> do DOM (o markdown já virou HTML antes dela) — o ambiente node
// padrão do projeto não tem document.
import { describe, it, expect } from 'vitest';
import { parseNumero, lerTabela, formatarValor } from './tableChart';

describe('parseNumero', () => {
  it('lê as formas que aparecem numa tabela minha de verdade', () => {
    // Todos estes saíram da tabela de custo Kimi x Pi desta sessão.
    expect(parseNumero('327')).toBe(327);
    expect(parseNumero('46,9M')).toBeCloseTo(46_900_000);
    expect(parseNumero('5,3M')).toBeCloseTo(5_300_000);
    expect(parseNumero('16k')).toBe(16_000);
  });

  it('resolve pt-BR e en pelo separador que vem por ÚLTIMO', () => {
    expect(parseNumero('1.234,56')).toBeCloseTo(1234.56);   // pt-BR
    expect(parseNumero('1,234.56')).toBeCloseTo(1234.56);   // en
    expect(parseNumero('46,9')).toBeCloseTo(46.9);          // decimal: <=2 dígitos depois
    expect(parseNumero('1.234.567')).toBe(1_234_567);       // 2+ pontos: só pode ser milhar
  });

  it('um ponto só fica DECIMAL — o caso é ambíguo e chutar milhar erra por 1000x', () => {
    // "1.234" é mil e duzentos em pt-BR e é 1,234 em inglês. Sem contexto não dá pra saber, então
    // vale a convenção da máquina. Documentado aqui pra ninguém "consertar" sem ler o porquê.
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
    expect(parseNumero('533 KB')).toBeNull();               // unidade junto não é número
    expect(parseNumero('—')).toBeNull();
  });

  it('versão de pacote NÃO é número', () => {
    // Pego ao vivo: numa tabela de bibliotecas a coluna "versão" se ofereceu como dado plotável,
    // porque "1.6.32" caía na regra de milhar e virava 1632. Milhar tem grupos de 3 dígitos.
    expect(parseNumero('1.6.32')).toBeNull();
    expect(parseNumero('4.5.1')).toBeNull();
    expect(parseNumero('0.0.7')).toBeNull();
    expect(parseNumero('1.234.567')).toBe(1_234_567);       // esse continua sendo milhar
  });
});

function tabelaDe(html: string): HTMLTableElement {
  const d = document.createElement('div');
  d.innerHTML = html;
  return d.querySelector('table')!;
}

describe('lerTabela', () => {
  const CUSTO = `<table>
    <tr><th></th><th>chamadas</th><th>bruto</th><th>cobrável</th><th>por chamada</th></tr>
    <tr><td>Kimi Code</td><td>327</td><td>46,9M</td><td>5,3M</td><td>16k</td></tr>
    <tr><td>Pi</td><td>769</td><td>132,5M</td><td>16,3M</td><td>21k</td></tr>
  </table>`;

  it('separa rótulos das colunas numéricas', () => {
    const t = lerTabela(tabelaDe(CUSTO))!;
    expect(t.rotulos).toEqual(['Kimi Code', 'Pi']);
    expect(t.colunas.map((c) => c.titulo)).toEqual(['chamadas', 'bruto', 'cobrável', 'por chamada']);
    expect(t.colunas[1].valores).toEqual([46_900_000, 132_500_000]);
  });

  it('recusa tabela de uma linha só — um número não é gráfico', () => {
    expect(lerTabela(tabelaDe('<table><tr><th>a</th></tr><tr><td>1</td></tr></table>'))).toBeNull();
  });

  it('recusa tabela sem nenhuma coluna numérica', () => {
    const t = tabelaDe(`<table>
      <tr><th>arquivo</th><th>o que faz</th></tr>
      <tr><td>a.ts</td><td>lê</td></tr>
      <tr><td>b.ts</td><td>escreve</td></tr></table>`);
    expect(lerTabela(t)).toBeNull();
  });

  it('coluna com UMA célula não-numérica inteira deixa de ser numérica', () => {
    // Célula "n/a" no meio: plotar isso como 0 inventaria dado.
    const t = lerTabela(tabelaDe(`<table>
      <tr><th>x</th><th>v</th></tr>
      <tr><td>a</td><td>1</td></tr>
      <tr><td>b</td><td>n/a</td></tr></table>`));
    expect(t).toBeNull();
  });
});

describe('formatarValor', () => {
  it('encurta na magnitude certa', () => {
    expect(formatarValor(46_900_000)).toBe('46,9M');
    expect(formatarValor(16_000)).toBe('16k');
    expect(formatarValor(327)).toBe('327');
  });
});
