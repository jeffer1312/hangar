import { parseNumero } from '@hangar/core';
import * as m from '../paraglide/messages';
import type { TabelaLida, ColunaNumerica } from '@hangar/core';

/** Lê uma <table> do DOM. Devolve null quando ela não tem dado plotável. */
export function lerTabela(tabela: HTMLTableElement): TabelaLida | null {
  const linhas = [...tabela.querySelectorAll('tr')];
  if (linhas.length < 2) return null;

  const titulos = [...linhas[0].querySelectorAll('th,td')].map((c) => c.textContent?.trim() ?? '');
  const corpo = linhas.slice(1).map((tr) => [...tr.querySelectorAll('th,td')].map((c) => c.textContent?.trim() ?? ''));
  if (corpo.length < 2) return null; // uma linha só não é gráfico, é um número

  // Coluna é numérica quando TODAS as células dela viram número.
  const nCols = Math.max(titulos.length, ...corpo.map((l) => l.length));
  const colunas: ColunaNumerica[] = [];
  let colRotulo = -1;
  for (let c = 0; c < nCols; c++) {
    const celulas = corpo.map((l) => l[c] ?? '');
    const nums = celulas.map(parseNumero);
    if (nums.every((n) => n !== null)) {
      colunas.push({ indice: c, titulo: titulos[c] || m.tabela_coluna({ n: c + 1 }), valores: nums as number[] });
    } else if (colRotulo < 0) {
      colRotulo = c; // a 1a não-numérica nomeia as linhas
    }
  }
  if (!colunas.length) return null;

  const rotulos = corpo.map((l, i) => (colRotulo >= 0 ? l[colRotulo] : '') || m.tabela_linha({ n: i + 1 }));
  return { rotulos, colunas };
}
