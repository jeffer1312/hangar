// Largura do navegador embutido: arrastável pela divisória esquerda e guardada (cp_nav_w).
// Mesma forma do ctxPanel — o Chat reserva a faixa no conteúdo (--cp-nav-w) e o NavegadorPane
// se posiciona nela, então os dois leem daqui e nunca divergem.

const CHAVE = 'cp_nav_w';

// Abaixo do MIN um browser não serve pra nada; o teto deixa uma coluna de chat legível mais o
// trilho da sidebar (que o navegador força ao abrir, via sidebarPin.setForced no Chat).
export const NAV_MIN = 400;
export const RESERVA_CHAT = 520;
export const RESERVA_TRILHO = 52;

function tetoLargura(): number {
  if (typeof window === 'undefined') return 920;
  return Math.max(NAV_MIN, window.innerWidth - RESERVA_CHAT - RESERVA_TRILHO);
}

function clampLargura(w: number): number {
  return Math.max(NAV_MIN, Math.min(tetoLargura(), w));
}

// Nasce grande: browser quer espaço (a coluna de contexto, em comparação, nasce em 264-340).
function larguraDefault(): number {
  if (typeof window === 'undefined') return 640;
  return clampLargura(Math.round(window.innerWidth * 0.42));
}

function carregarLargura(): number {
  try {
    const salva = Number(localStorage.getItem(CHAVE));
    if (Number.isFinite(salva) && salva > 0) return clampLargura(salva);
  } catch {
    /* sem storage: default */
  }
  return clampLargura(larguraDefault());
}

// `resizing` aqui e não num $state do NavegadorPane: o flag precisa sobreviver à desmontagem no
// meio do arrasto (trocar de sessão remonta o Chat) — mesmo motivo do ctxPanel.
export const navegadorPanel = $state({
  largura: carregarLargura(),
  resizing: false,
});

// O painel cola na direita: largura = janela - clientX (espelho do arrastarLargura do ctxPanel).
export function arrastarNav(clientX: number): void {
  if (typeof window === 'undefined') return;
  navegadorPanel.largura = clampLargura(window.innerWidth - clientX);
}

export function salvarNav(): void {
  try {
    localStorage.setItem(CHAVE, String(navegadorPanel.largura));
  } catch {
    /* sem storage: vale só nesta sessão */
  }
}
