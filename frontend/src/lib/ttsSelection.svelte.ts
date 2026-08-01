import { falavelDaSelecaoComCodigo } from './selectionText';
import { rafThrottle } from './rafThrottle';

// Estado da selecao corrente, gravado no selectionchange.
//
// Por que gravar antes e nao ler no clique: o pointerdown no botao COLAPSA a selecao (e no iOS leva
// o menu nativo junto), entao getSelection() dentro do onclick devolve string vazia. Quem le o
// botao e este estado, nunca o DOM.

let texto = $state('');
// Blocos de <pre> da selecao (fase 2: narracao guiada). Populado junto do texto, pelo mesmo motivo:
// o pointerdown ja colapsou a selecao quando o preset "Explicar o codigo" e escolhido.
let blocos = $state<string[]>([]);
let x = $state(0);
let y = $state(0);
// Altura MEDIDA do painel (TtsSelectionPill, via ResizeObserver) — mesmo padrao do ttsPlayer.barH.
// A fase 1 cravava 52px direto no App.svelte; a fase 2 acrescenta presets + campo livre, que
// crescem o painel bem alem disso (e o erro da Groq, como o da ElevenLabs, pode quebrar em
// varias linhas num celular estreito).
let panelH = $state(0);
// Espelha o `engajado` da TtsSelectionPill: quem decide se o painel esta NA TELA, porque focar o
// campo de instrucao colapsa a Selection API (ativa vira false no meio da digitacao) e o painel
// continua visivel — ver comentario da propria pill. --cp-tts-h (App.svelte) precisa desta
// condicao, nao de `ativa`, senao as pills do Chat descem por baixo do painel aberto assim que o
// usuario toca no campo livre.
let engajado = $state(false);

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
    if (!sel || sel.isCollapsed || !dentroDoChat(sel)) { texto = ''; blocos = []; return; }
    const { texto: t, blocos: bs } = falavelDaSelecaoComCodigo(sel);
    if (!t) { texto = ''; blocos = []; return; }
    texto = t;
    blocos = bs;
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
  get blocos() { return blocos; },
  get temCodigo() { return blocos.length > 0; },
  get x() { return x; },
  get y() { return y; },
  get ativa() { return texto.length > 0; },
  get panelH() { return panelH; },
  get engajado() { return engajado; },
  limpar() { texto = ''; blocos = []; },
  /** Publicado pela TtsSelectionPill (dona da medicao). 0 quando ela desmonta. */
  setPanelH(h: number) { panelH = h; },
  /** Publicado pela TtsSelectionPill (dona do estado) junto de cada mudanca do seu `engajado`. */
  setEngajado(v: boolean) { engajado = v; },
};
