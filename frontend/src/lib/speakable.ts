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
// Cartao de ferramenta (ToolCard.svelte, raiz `.tool-row`): saida de Bash/comando, nao prosa. Uma
// selecao que ATRAVESSA um cartao (comeca na mensagem, passa por cima do card, continua depois) nao
// e barrada por `dentroDoChat` — o ancora dela e a mensagem, nao o card. Por isso o corte tem que
// ser aqui, no que foi CLONADO, igual ao <pre>: nao ha como resolver isso olhando so onde a selecao
// termina.
const MARCADOR_FERRAMENTA = ' saída de comando omitida. ';

// Elementos de bloco do renderMarkdown (markdown.ts: `out.join('')`, sem separador nenhum entre
// eles). Sem uma quebra aqui, "## Passo 1" + o paragrafo seguinte viram "Passo 1Fazer X" no
// textContent achatado — o servidor (tts_text.py) so tem como transformar quebra de bloco em
// pausa se o front mandar a quebra.
const BLOCOS = 'p,h1,h2,h3,h4,h5,h6,li,blockquote,tr,td,th,pre,div';

// Fase 2 (narracao guiada): extrai TAMBEM o codigo que o marcador descarta, pra Groq poder
// explicar a logica em vez de so saber que "havia codigo ali". Uma unica implementacao — textoFalavel
// so devolve o campo .texto dela — pra nao arriscar as duas formas divergirem e quebrar o hash do
// cache da fase 1 (que depende do .texto byte a byte).
function extrair(raiz: HTMLElement): { texto: string; blocos: string[] } {
  // Clone: trocar o conteudo do <pre> no no original apagaria o codigo da tela do usuario.
  const copia = raiz.cloneNode(true) as HTMLElement;
  const blocos: string[] = [];
  copia.querySelectorAll('pre').forEach((p) => {
    blocos.push(p.textContent ?? '');
    p.replaceWith(document.createTextNode(MARCADOR));
  });
  copia.querySelectorAll('.tool-row').forEach((c) => {
    c.replaceWith(document.createTextNode(MARCADOR_FERRAMENTA));
  });
  // Marca o FIM de cada bloco com \n antes de achatar (ordem importa: <pre> ja virou texto acima,
  // entao um <pre> dentro de <li> ganha a quebra do <li> em volta, no lugar certo).
  copia.querySelectorAll(BLOCOS).forEach((b) => {
    b.appendChild(document.createTextNode('\n'));
  });
  const texto = (copia.textContent ?? '')
    .replace(/[^\S\n]+/g, ' ')   // colapsa espaco/tab, preserva \n
    .replace(/\n+/g, '\n')
    .trim();
  return { texto, blocos };
}

export function textoFalavel(raiz: HTMLElement): string {
  return extrair(raiz).texto;
}

/** Igual a textoFalavel, mas devolve tambem os blocos de <pre> que o marcador substituiu. */
export function textoFalavelComCodigo(raiz: HTMLElement): { texto: string; blocos: string[] } {
  return extrair(raiz);
}
