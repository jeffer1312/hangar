import { copyText } from './clipboard';

// Acoes dos blocos de codigo (copiar/expandir), GLOBAIS: um listener no document cobre qualquer
// {@html} de renderMarkdown — bolha do chat, PairSheet, ActivitySheet, plano, hover preview.
// Antes o copy-btn so tinha handler no AssistantBubble: nas outras telas o botao era morto.
//
// .svelte.ts porque o estado do overlay usa runes fora de componente (mesmo padrao do ttsPlayer).

let atual = $state<{ code: string; lang: string } | null>(null);

/** Estado do overlay fullscreen de codigo (montado uma vez no App.svelte). */
export const codeOverlay = {
  get atual() { return atual; },
  fechar() { atual = null; },
};

/** Um listener pro app inteiro. Devolve a funcao de cleanup (App.svelte, onMount). */
export function iniciarCodeActions(): () => void {
  function aoClicar(e: MouseEvent) {
    const btn = (e.target as HTMLElement).closest('.copy-btn, .expand-btn');
    if (!btn) return;
    const bloco = btn.closest('.code-block');
    const pre = bloco?.querySelector('pre');
    if (!pre) return;

    if (btn.classList.contains('copy-btn')) {
      copyText(pre.textContent ?? '');
      btn.classList.add('copied');
      setTimeout(() => btn.classList.remove('copied'), 1200);
      return;
    }
    // expand-btn: abre o overlay com o TEXTO cru (o highlight roda de novo la dentro — assim o
    // overlay nao depende de o bloco original ja ter sido colorido).
    const lang = pre.querySelector('code')?.className.match(/language-([\w-]+)/)?.[1] ?? '';
    atual = { code: pre.textContent ?? '', lang };
  }

  function aoTeclar(e: KeyboardEvent) {
    if (e.key === 'Escape' && atual) {
      e.stopImmediatePropagation();   // o Esc do overlay nao fecha sheets/paineis atras dele
      atual = null;
    }
  }

  // WINDOW com capture, nao document: CommitMenu/AttachmentsSheet escutam Esc em capture na window
  // com stopImmediatePropagation, e capture da window roda ANTES do capture do document — o Esc
  // fechava o menu e o overlay (aberto por cima) ficava aberto. O App monta antes de tudo, entao
  // este listener registra primeiro e ganha.
  document.addEventListener('click', aoClicar);
  window.addEventListener('keydown', aoTeclar, true);
  return () => {
    document.removeEventListener('click', aoClicar);
    window.removeEventListener('keydown', aoTeclar, true);
  };
}
