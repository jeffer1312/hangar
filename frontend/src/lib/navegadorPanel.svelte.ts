// Largura do navegador embutido: arrastável pela divisória esquerda e guardada (cp_nav_w).
// Mesma forma do ctxPanel — o Chat reserva a faixa no conteúdo (--cp-nav-w) e o NavegadorPane
// se posiciona nela, então os dois leem daqui e nunca divergem.
import { sidebarPin } from './sidebarPin.svelte';

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
// `abertos`: sessões com navegador aberto (chave workspaceSessionKey → url atual, '' = ainda não
// navegou). Vive aqui porque o Chat remonta por key a cada troca de sessão — um $state local
// esqueceria que a sessão X tem navegador aberto. Persistido pra um reload da página reexibir
// (o view continua vivo no main; se o SHELL reiniciou, o front reabre com esta url).
//
// ORDEM IMPORTA: as consts e funções de carga ficam ANTES do $state — o inicializador roda na
// linha dele, e uma const declarada depois está no TDZ (o try/catch do carregarAbertos engolia o
// ReferenceError e o store nascia vazio mesmo com localStorage cheio — o painel não remontava
// depois de reload).
const CHAVE_ABERTOS = 'cp_nav_abertos';

function carregarAbertos(): Record<string, string> {
  try {
    const j = JSON.parse(localStorage.getItem(CHAVE_ABERTOS) || '{}');
    return j && typeof j === 'object' && !Array.isArray(j) ? j : {};
  } catch {
    return {};
  }
}

export const navegadorPanel = $state({
  largura: carregarLargura(),
  resizing: false,
  abertos: carregarAbertos(),
});

function salvarAbertos(): void {
  try {
    localStorage.setItem(CHAVE_ABERTOS, JSON.stringify(navegadorPanel.abertos));
  } catch {
    /* sem storage: vale só nesta sessão */
  }
}

export function marcarNavAberto(chave: string): void {
  if (!(chave in navegadorPanel.abertos)) {
    navegadorPanel.abertos[chave] = '';
    salvarAbertos();
    syncSidebar();
  }
}

export function atualizarNavUrl(chave: string, url: string): void {
  if (chave in navegadorPanel.abertos) {
    navegadorPanel.abertos[chave] = url;
    salvarAbertos();
  }
}

export function fecharNav(chave: string): void {
  delete navegadorPanel.abertos[chave];
  salvarAbertos();
  syncSidebar();
}

// ── Sidebar com navegador aberto ───────────────────────────────────────────
// Quando o PRIMEIRO navegador abre, a sidebar colapsa pro trilho; quando o ÚLTIMO fecha, volta
// como estava. SEM override forçado (diferente do Board/Canvas): o fold continua vivo e a sidebar
// expandida se comporta como sempre — empurra o conteúdo normal, o ResizeObserver do NavegadorPane
// re-mede e o view acompanha. Se ele mexeu no fold no meio, a escolha dele fica.
let sidebarAntes: boolean | null = null;

function syncSidebar(): void {
  const algum = Object.keys(navegadorPanel.abertos).length > 0;
  if (algum && sidebarAntes === null) {
    sidebarAntes = sidebarPin.preferred;
    sidebarPin.setUser(true);
  } else if (!algum && sidebarAntes !== null) {
    if (sidebarPin.preferred === true) sidebarPin.setUser(sidebarAntes);
    sidebarAntes = null;
  }
}

// O colapso é EFEITO do conjunto de abertos, não das chamadas de abrir/fechar: o remount após
// um reload da página não passa por marcarNavAberto e sem isto a sidebar acordava expandida com
// o navegador aberto. $effect.root porque store de módulo não tem componente pra ancorar o efeito.
$effect.root(() => {
  $effect(() => {
    void Object.keys(navegadorPanel.abertos).length;   // reage ao CONJUNTO, não às urls
    syncSidebar();
  });
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
