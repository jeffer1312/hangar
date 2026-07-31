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

export function textoFalavel(raiz: HTMLElement): string {
  // Clone: trocar o conteudo do <pre> no no original apagaria o codigo da tela do usuario.
  const copia = raiz.cloneNode(true) as HTMLElement;
  copia.querySelectorAll('pre').forEach((p) => {
    p.replaceWith(document.createTextNode(MARCADOR));
  });
  return (copia.textContent ?? '').replace(/\s+/g, ' ').trim();
}
