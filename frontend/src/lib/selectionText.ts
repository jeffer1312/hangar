import { textoFalavel } from './speakable';

// Texto falavel de uma selecao que pode cruzar bolhas, paragrafos e tool cards.
//
// cloneContents() devolve um fragmento com a marcacao preservada, entao o MESMO textoFalavel do
// botao da bolha vale aqui — e isso e o que faz o hash do cache bater entre os dois caminhos.
// Selection.toString() nao serviria: ele ja achata tudo, e o <pre> viraria codigo soletrado.
export function falavelDaSelecao(sel: Selection | null): string {
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return '';
  const hospedeiro = document.createElement('div');
  for (let i = 0; i < sel.rangeCount; i++) {
    hospedeiro.appendChild(sel.getRangeAt(i).cloneContents());
  }
  return textoFalavel(hospedeiro);
}
