import { falavelDaSelecao } from './selectionText';
import { rafThrottle } from './rafThrottle';

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
  // selectionchange dispara em rajada durante o arraste (dezenas de vezes por segundo). Cada
  // disparo faria falavelDaSelecao -> cloneContents() +, dentro de textoFalavel, um SEGUNDO clone
  // completo + querySelectorAll('pre') + regex na string inteira — caro demais pra rodar sem
  // throttle. rafThrottle (lib/rafThrottle.ts, testado ali) agenda no maximo 1 quadro por vez; o
  // quadro le a selecao ATUAL quando roda (nao a do disparo que agendou), entao o ULTIMO estado
  // sempre vence — inclusive o disparo do mouseup, que ainda cai dentro do mesmo quadro pendente.
  const processar = () => {
    const sel = document.getSelection();
    if (!sel || sel.isCollapsed || !dentroDoChat(sel)) { texto = ''; return; }
    const t = falavelDaSelecao(sel);
    if (!t) { texto = ''; return; }
    texto = t;
    const r = sel.getRangeAt(sel.rangeCount - 1).getBoundingClientRect();
    x = r.right;
    y = r.bottom;
  };

  const { agendar, cancelar } = rafThrottle(processar);
  document.addEventListener('selectionchange', agendar);
  return () => {
    document.removeEventListener('selectionchange', agendar);
    cancelar();
  };
}

export const ttsSelection = {
  get texto() { return texto; },
  get x() { return x; },
  get y() { return y; },
  get ativa() { return texto.length > 0; },
  limpar() { texto = ''; },
};
