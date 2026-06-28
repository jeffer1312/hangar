// Janela de render do chat: monta SO os ultimos N eventos (a cauda), nunca o transcript inteiro.
// Contagem-A-PARTIR-DO-FIM (relativa): um prepend futuro (paginacao backend / fix B) nao corrompe a
// janela, porque ela e sempre medida do fim.

/** Indice inicial (inclusivo) da fatia visivel, dado o fim da janela e o tamanho. Clampa em 0. */
export function windowStartFor(windowEnd: number, size: number): number {
  return Math.max(0, windowEnd - size);
}

/** Proximo fim de janela:
 *  - encolheu (reset / /clear) -> re-ancora na cauda nova (senao a slice fica fora do array = chat em branco);
 *  - colado no fim -> acompanha a cauda (remonta o topo SO com o usuario no fundo = sem pulo);
 *  - rolado pra cima -> congela. */
export function nextWindowEnd(atBottom: boolean, len: number, windowEnd: number): number {
  if (windowEnd > len) return len;   // transcript encolheu: clampa
  if (atBottom) return len;          // gruda no fim: janela segue a cauda
  return windowEnd;                  // lendo historico: congela
}
