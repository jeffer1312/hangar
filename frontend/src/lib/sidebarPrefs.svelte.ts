// Preferencia da barra lateral (desktop): que ALTURA ela ocupa quando esta aberta.
//
// Existiu tambem uma chave "manter aberta" — apagada em 2026-07-30 por duplicar o pin. O botao de
// recolher da propria barra ja fazia isso desde sempre (`expanded = !collapsed || hovering`), entao
// a opcao criava um segundo controle pro mesmo estado e deixava o icone obvio como clique morto. O
// que faltava de verdade era so poder escolher a altura: com o pin aberto e 'content', a barra fica
// no formato de dock flutuante que antes so aparecia durante o hover.
//
// .svelte.ts porque usa runes fora de componente — mesmo padrao do sessionsStore.

const HEIGHT_KEY = 'cp_sidebar_height';

// 'full' = ocupa a altura toda (o comportamento de sempre com o pin aberto).
// 'content' = altura do conteudo, dock flutuante.
export type SidebarHeight = 'full' | 'content';

function loadHeight(): SidebarHeight {
  try { return localStorage.getItem(HEIGHT_KEY) === 'content' ? 'content' : 'full'; } catch { return 'full'; }
}

let height = $state<SidebarHeight>(loadHeight());

export const sidebarPrefs = {
  get height() { return height; },
  set height(v: SidebarHeight) {
    height = v;
    try { if (v === 'content') localStorage.setItem(HEIGHT_KEY, 'content'); else localStorage.removeItem(HEIGHT_KEY); }
    catch { /* modo privado: vale pela sessao */ }
  },
};
