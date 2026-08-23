// Tabela markdown -> gráfico, sob demanda.
//
// Progressive enhancement em cima do HTML que o renderMarkdown já produziu, no MESMO molde do
// highlightCodeBlocks: roda depois da montagem, é idempotente e não toca no pipeline do markdown.
// Sem isto, pôr um gráfico dentro de uma mensagem exigiria fatiar o markdown em componentes.
//
// Por que UMA coluna de cada vez, e não barra agrupada: numa tabela real as colunas têm unidades
// diferentes. Na tabela de custo desta máquina — Kimi Code 327 chamadas / 46,9M bruto — plotar
// "chamadas" e "bruto" juntos põe 327 e 46.900.000 na mesma escala: a primeira barra vira um traço
// invisível e o gráfico mente. Então o gráfico mostra uma coluna, e quem lê escolhe qual.

import { intlLocale } from './i18n';
import * as m from './paraglide/messages';

export interface ColunaNumerica {
  indice: number;
  titulo: string;
  valores: number[];
}

export interface TabelaLida {
  rotulos: string[]; // a 1a coluna não-numérica: o nome de cada linha
  colunas: ColunaNumerica[];
}

// "46,9M" / "5,3M" / "16k" / "1.234,56" / "R$ 1.200" / "-3,5%" -> número.
// Devolve null quando a célula não é numérica — é isso que separa rótulo de dado.
export function parseNumero(bruto: string): number | null {
  const s = bruto.trim();
  if (!s) return null;
  // Sufixo de magnitude (k/M/B), com ou sem espaço antes.
  const mat = s.match(/^([+-]?[\d.,\s]+)\s*([kKmMbB])?\s*%?$/);
  if (!mat) return null;
  let corpo = mat[1].replace(/\s/g, '');
  // Formato pt-BR ("1.234,56") vs en ("1,234.56"): manda quem aparece por ÚLTIMO como separador
  // decimal. Sem isto, "46,9" virava 469 e "1.234" virava 1.234.
  const ultimaVirgula = corpo.lastIndexOf(',');
  const ultimoPonto = corpo.lastIndexOf('.');
  if (ultimaVirgula >= 0 && ultimoPonto >= 0) {
    if (ultimaVirgula > ultimoPonto) corpo = corpo.replace(/\./g, '').replace(',', '.');
    else corpo = corpo.replace(/,/g, '');
  } else if (ultimaVirgula >= 0) {
    // Só vírgula: decimal se sobram <= 2 dígitos depois dela ("46,9"); senão é milhar ("1,234").
    const depois = corpo.length - ultimaVirgula - 1;
    corpo = depois <= 2 ? corpo.replace(',', '.') : corpo.replace(/,/g, '');
  } else if ((corpo.match(/\./g) ?? []).length > 1) {
    // Só ponto, e mais de um: milhar ("1.234.567") SE todo grupo depois do 1o tiver exatamente 3
    // dígitos. Sem essa checagem, "1.6.32" (versão de pacote) virava 1632 — pego ao vivo numa
    // tabela de bibliotecas, onde a coluna "versão" passou a se oferecer como dado plotável.
    const g = corpo.split('.');
    if (!g.slice(1).every((p) => /^\d{3}$/.test(p))) return null;
    corpo = g.join('');
  }
  // Só ponto e UM ponto ("1.234") fica ambíguo de verdade — mil e duzentos em pt-BR, 1,234 em
  // inglês. Fica como DECIMAL, que é a convenção da máquina: chutar milhar erraria por 1000x um
  // número que o autor escreveu certo. Quem quer milhar aqui escreve "1234" ou "1,234".
  const n = Number(corpo);
  if (!Number.isFinite(n)) return null;
  const mult = mat[2] ? { k: 1e3, m: 1e6, b: 1e9 }[mat[2].toLowerCase() as 'k' | 'm' | 'b'] : 1;
  return n * (mult ?? 1);
}

/** Lê tabelas markdown (GFM) do texto. Devolve array de tabelas plotáveis (zero a N). */
export function lerTabelaMarkdown(md: string): TabelaLida[] {
  const linhas = md.split('\n');
  const out: TabelaLida[] = [];
  for (let i = 0; i < linhas.length; ) {
    const linha = linhas[i].trim();
    // Cabeçalho: começa e termina com |
    if (!/^\|(.+)\|$/.test(linha)) {
      i++;
      continue;
    }
    const sep = (linhas[i + 1] ?? '').trim();
    if (!/^\|[\s:|-]+\|$/.test(sep)) {
      i++;
      continue;
    }
    const headerCells = linha
      .slice(1, -1)
      .split('|')
      .map((c) => c.trim());
    const corpo: string[][] = [];
    let j = i + 2;
    while (j < linhas.length) {
      const r = linhas[j].trim();
      if (!/^\|(.+)\|$/.test(r)) break;
      const cells = r
        .slice(1, -1)
        .split('|')
        .map((c) => c.trim());
      corpo.push(cells);
      j++;
    }
    if (corpo.length < 2) {
      i = j;
      continue; // uma linha só não é gráfico
    }
    const nCols = Math.max(headerCells.length, ...corpo.map((l) => l.length));
    const colunas: ColunaNumerica[] = [];
    let colRotulo = -1;
    for (let c = 0; c < nCols; c++) {
      const celulas = corpo.map((l) => l[c] ?? '');
      const nums = celulas.map(parseNumero);
      if (nums.every((n) => n !== null)) {
        colunas.push({ indice: c, titulo: headerCells[c] || m.tabela_coluna({ n: c + 1 }), valores: nums as number[] });
      } else if (colRotulo < 0) {
        colRotulo = c;
      }
    }
    if (colunas.length) {
      const rotulos = corpo.map((l, idx) => (colRotulo >= 0 ? l[colRotulo] : '') || m.tabela_linha({ n: idx + 1 }));
      out.push({ rotulos, colunas });
    }
    i = j;
  }
  return out;
}

/** Formata pro eixo: 46900000 -> "46,9M". */
export function formatarValor(v: number): string {
  const abs = Math.abs(v);
  if (abs >= 1e9) return (v / 1e9).toLocaleString(intlLocale(), { maximumFractionDigits: 1 }) + 'B';
  if (abs >= 1e6) return (v / 1e6).toLocaleString(intlLocale(), { maximumFractionDigits: 1 }) + 'M';
  if (abs >= 1e3) return (v / 1e3).toLocaleString(intlLocale(), { maximumFractionDigits: 1 }) + 'k';
  return v.toLocaleString(intlLocale(), { maximumFractionDigits: 2 });
}
