// Texto falavel a partir do DOM JA RENDERIZADO da bolha.
//
// Por que daqui e nao do markdown cru: o renderMarkdown ja consumiu ##, ** e backtick, entao o que
// esta na tela e o que a pessoa quer ouvir. Mandar o markdown cru pelo botao da mensagem e o texto
// do DOM pela selecao geraria hashes de cache DIFERENTES pro mesmo conteudo, e metade da limpeza do
// servidor seria codigo morto num dos dois caminhos.
//
// O marcador de bloco de codigo tambem so pode ser decidido aqui: <pre> e elemento; depois de
// achatar em string nao ha como saber onde comecava.

const MARCADOR = ' trecho de código omitido. ';

// Elementos de bloco do renderMarkdown (markdown.ts: `out.join('')`, sem separador nenhum entre
// eles). Sem uma quebra aqui, "## Passo 1" + o paragrafo seguinte viram "Passo 1Fazer X" no
// textContent achatado — o servidor (tts_text.py) so tem como transformar quebra de bloco em
// pausa se o front mandar a quebra.
const BLOCOS = 'p,h1,h2,h3,h4,h5,h6,li,blockquote,tr,td,th,pre,div';

export function textoFalavel(raiz: HTMLElement): string {
  // Clone: trocar o conteudo do <pre> no no original apagaria o codigo da tela do usuario.
  const copia = raiz.cloneNode(true) as HTMLElement;
  copia.querySelectorAll('pre').forEach((p) => {
    p.replaceWith(document.createTextNode(MARCADOR));
  });
  // Marca o FIM de cada bloco com \n antes de achatar (ordem importa: <pre> ja virou texto acima,
  // entao um <pre> dentro de <li> ganha a quebra do <li> em volta, no lugar certo).
  copia.querySelectorAll(BLOCOS).forEach((b) => {
    b.appendChild(document.createTextNode('\n'));
  });
  return (copia.textContent ?? '')
    .replace(/[^\S\n]+/g, ' ')   // colapsa espaco/tab, preserva \n
    .replace(/\n+/g, '\n')
    .trim();
}
