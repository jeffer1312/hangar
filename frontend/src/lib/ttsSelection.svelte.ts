import { falavelDaSelecao } from './selectionText';

// Estado da selecao corrente, gravado no selectionchange.
//
// Por que gravar antes e nao ler no clique: o pointerdown no botao COLAPSA a selecao (e no iOS leva
// o menu nativo junto), entao getSelection() dentro do onclick devolve string vazia. Quem le o
// botao e este estado, nunca o DOM.

let texto = $state('');
let x = $state(0);
let y = $state(0);

function dentroDoChat(sel: Selection): boolean {
  const no = sel.anchorNode;
  const el = no instanceof Element ? no : no?.parentElement ?? null;
  // So oferece leitura dentro de conteudo de mensagem — nao no composer nem em campo de config.
  return !!el?.closest('.prose, .bubble');
}

export function iniciarCapturaDeSelecao(): () => void {
  const aoMudar = () => {
    const sel = document.getSelection();
    if (!sel || sel.isCollapsed || !dentroDoChat(sel)) { texto = ''; return; }
    const t = falavelDaSelecao(sel);
    if (!t) { texto = ''; return; }
    texto = t;
    const r = sel.getRangeAt(sel.rangeCount - 1).getBoundingClientRect();
    x = r.right;
    y = r.bottom;
  };
  document.addEventListener('selectionchange', aoMudar);
  return () => document.removeEventListener('selectionchange', aoMudar);
}

export const ttsSelection = {
  get texto() { return texto; },
  get x() { return x; },
  get y() { return y; },
  get ativa() { return texto.length > 0; },
  limpar() { texto = ''; },
};
