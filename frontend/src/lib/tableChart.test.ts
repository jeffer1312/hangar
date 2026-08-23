// @vitest-environment happy-dom
import { describe, it, expect, beforeEach } from 'vitest';
import { lerTabela } from './tableChartDom';
import { overwriteGetLocale } from '../paraglide/runtime';

beforeEach(() => overwriteGetLocale(() => 'pt'));

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
    const t = lerTabela(tabelaDe(`<table>
      <tr><th>x</th><th>v</th></tr>
      <tr><td>a</td><td>1</td></tr>
      <tr><td>b</td><td>n/a</td></tr></table>`));
    expect(t).toBeNull();
  });
});
