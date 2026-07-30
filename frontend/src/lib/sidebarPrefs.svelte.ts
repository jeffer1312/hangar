// Preferencias da barra lateral (desktop). Duas chaves INDEPENDENTES, porque hoje "estar aberta" e
// "que altura ocupar" andam grudadas no pin: com o pin recolhido a barra vira um dock de altura de
// conteudo que so abre no hover; com o pin aberto vira uma peca de ponta a ponta. Quem gosta do dock
// aberto o tempo todo nao tinha como pedir isso.
//
// .svelte.ts porque usa runes fora de componente — mesmo padrao do sessionsStore.

const OPEN_KEY = 'cp_sidebar_always_open';
const HEIGHT_KEY = 'cp_sidebar_height';

// 'full' = ocupa a altura toda (o comportamento de sempre com o pin aberto).
// 'content' = altura do conteudo, dock flutuante — o formato que so aparecia no hover.
export type SidebarHeight = 'full' | 'content';

function loadOpen(): boolean {
  try { return localStorage.getItem(OPEN_KEY) === '1'; } catch { return false; }
}
function loadHeight(): SidebarHeight {
  try { return localStorage.getItem(HEIGHT_KEY) === 'content' ? 'content' : 'full'; } catch { return 'full'; }
}

let alwaysOpen = $state(loadOpen());
let height = $state<SidebarHeight>(loadHeight());

export const sidebarPrefs = {
  get alwaysOpen() { return alwaysOpen; },
  set alwaysOpen(v: boolean) {
    alwaysOpen = v;
    try { if (v) localStorage.setItem(OPEN_KEY, '1'); else localStorage.removeItem(OPEN_KEY); }
    catch { /* modo privado: vale pela sessao */ }
  },
  get height() { return height; },
  set height(v: SidebarHeight) {
    height = v;
    try { if (v === 'content') localStorage.setItem(HEIGHT_KEY, 'content'); else localStorage.removeItem(HEIGHT_KEY); }
    catch { /* modo privado: vale pela sessao */ }
  },
};
