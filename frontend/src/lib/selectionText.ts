import { textoFalavel, textoFalavelComCodigo } from './speakable';

// Texto falavel de uma selecao que pode cruzar bolhas, paragrafos e tool cards.
//
// cloneContents() devolve um fragmento com a marcacao preservada, entao o MESMO textoFalavel do
// botao da bolha vale aqui — e isso e o que faz o hash do cache bater entre os dois caminhos.
// Selection.toString() nao serviria: ele ja achata tudo, e o <pre> viraria codigo soletrado.
export function falavelDaSelecao(sel: Selection | null): string {
  return falavelDaSelecaoComCodigo(sel).texto;
}

/** Igual a falavelDaSelecao, mas devolve tambem os blocos de codigo da selecao (fase 2: narracao
 * guiada precisa deles pra Groq explicar a logica em vez de so ler "trecho de codigo omitido"). */
export function falavelDaSelecaoComCodigo(sel: Selection | null): { texto: string; blocos: string[] } {
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return { texto: '', blocos: [] };
  const hospedeiro = document.createElement('div');
  for (let i = 0; i < sel.rangeCount; i++) {
    hospedeiro.appendChild(sel.getRangeAt(i).cloneContents());
  }
  return textoFalavelComCodigo(hospedeiro);
}
