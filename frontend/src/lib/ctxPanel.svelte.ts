// Estado do painel de contexto (a coluna da direita, no desktop): aberto ou recolhido em trilho.
//
// Vive fora dos dois componentes porque DOIS lados precisam do mesmo valor: o painel, pra desenhar
// o trilho, e o Chat, pra devolver a largura ao texto (`--ctx-w`). Sem uma fonte única, um dos dois
// ficaria com a faixa errada — o clássico "recolheu mas o chat não cresceu".
//
// Persiste em localStorage: reabrir a tela deve respeitar a escolha, como já acontece com a barra
// da esquerda (`loadPin` no Sidebar).

const CHAVE = 'cp_ctx_recolhido';
const CHAVE_LARGURA = 'cp_ctx_w';

// Largura da faixa reservada pelo Chat. Aberto: vem de `ctxPanel.largura` (arrastável, guardada);
// LARGURA_ABERTO é o default quando nada foi salvo. Recolhido: só a aba da borda, o que devolve
// ~230px de leitura ao chat.
export const LARGURA_ABERTO = 264;
export const LARGURA_TRILHO = 34;

// Larguras do painel aberto. MIN: abaixo disso a árvore de arquivos e a lista de tarefas ficam
// ilegíveis (o painel nasceu em 264 e 240 ainda lê caminho e título). MAX: acima disso o visor de
// arquivo ao lado fica estreito demais para um diff. RESERVA_VISOR: o mínimo do diff medido (558
// legível, 450 espremido). RESERVA_NAV: folga da sidebar da esquerda (default 270; se ela estiver
// menor/recolhida o teto só fica um pouco conservador, nunca espreme). Aplicadas como teto pela
// janela na carga E no arrasto, senão quem salvou numa tela grande abre numa menor com o chat
// espremido.
// (Sem RESERVA_CHAT de propósito: a sobreposição do composer não é o teto ser largo demais — é o
// --recuo-esq do Chat roubando largura do cartão; o item A (recuo zero) resolveu e o teto de 440
// em 1280 passou a caber com folga. Medido na rodada 2.)
export const LARGURA_MIN = 240;
export const LARGURA_MAX = 560;
export const RESERVA_VISOR = 560;
export const RESERVA_NAV = 280;

function tetoLargura(): number {
  if (typeof window === 'undefined') return LARGURA_MAX;
  return Math.min(LARGURA_MAX, window.innerWidth - RESERVA_VISOR - RESERVA_NAV);
}

function clampLargura(w: number): number {
  return Math.max(LARGURA_MIN, Math.min(tetoLargura(), w));
}

// Default que cresce com a tela: replica os degraus que o Chat tinha em media query (300px em
// >=1600, 340px em >=1900) — telas grandes nascem com painel maior; depois o usuário arrasta.
function larguraDefault(): number {
  if (typeof window === 'undefined') return LARGURA_ABERTO;
  if (window.innerWidth >= 1900) return 340;
  if (window.innerWidth >= 1600) return 300;
  return LARGURA_ABERTO;
}

function carregarLargura(): number {
  try {
    const salva = Number(localStorage.getItem(CHAVE_LARGURA));
    if (Number.isFinite(salva) && salva > 0) return clampLargura(salva);
  } catch {
    /* sem storage: default */
  }
  return clampLargura(larguraDefault());
}

function carregar(): boolean {
  try {
    return localStorage.getItem(CHAVE) === '1';
  } catch {
    return false; // navegador sem storage (modo privado/embutido): o padrão é aberto
  }
}

export const ctxPanel = $state({
  recolhido: carregar(),
  largura: carregarLargura(),
  // Arrasto em curso (classe .resizing do painel). Vive aqui — e nao num `let $state` do
  // DesktopSessionContext — porque o componente tem uma prop chamada `state` e o compilador
  // do Svelte confunde o rune `$state` com subscricao de store (store_rune_conflict). O
  // objeto do store e deep-reactive, entao a classe acompanha do mesmo jeito.
  resizing: false,
  // Aba ativa do painel (Contexto | Arquivos). Vive aqui por fora dos componentes porque o App
  // remonta o Chat (e o painel) por key a cada troca de sessao — um $state local devolveria
  // o usuario pra Contexto com a aba Arquivos aberta, e a regua "a aba sobrevive a troca de
  // sessao" morre sem erro nenhum. Nao persiste em localStorage: reabrir a tela volta pra
  // Contexto, que e o estado inicial do produto.
  // (Cuidado: a chave literal de bloco no comentario quebraria a varredura de string crua,
  // que trata arquivo .ts como markup.)
  aba: 'contexto' as 'contexto' | 'arquivos',
});

export function alternarCtxPanel(): void {
  ctxPanel.recolhido = !ctxPanel.recolhido;
  try {
    localStorage.setItem(CHAVE, ctxPanel.recolhido ? '1' : '0');
  } catch {
    /* sem storage: vale só nesta sessão */
  }
}

// Largura aplicada durante o arrasto da divisória (mesma régua da Sidebar, espelhada: lá o painel
// cola na esquerda e `largura = clientX`; aqui cola na direita e `largura = janela - clientX`).
// Clampa pelo mesmo teto da carga, então arrastar não espreme o visor ao lado.
export function arrastarLargura(clientX: number): void {
  if (typeof window === 'undefined') return;
  ctxPanel.largura = clampLargura(window.innerWidth - clientX);
}

export function salvarLargura(): void {
  try {
    localStorage.setItem(CHAVE_LARGURA, String(ctxPanel.largura));
  } catch {
    /* sem storage: vale só nesta sessão */
  }
}

// Reaplica a largura quando a JANELA muda de tamanho: o teto é função da largura da janela, então
// encolher a janela invalida uma largura que era legítima. Com escolha salva, relê o `cp_ctx_w` e
// clampa pela janela atual — o MESMO comportamento da carga: encolher reduz na hora, crescer de
// volta restaura a escolha grande (veio do localStorage, que não foi tocado). SEM escolha salva,
// o default acompanha a tela — era o que a media query do Chat fazia (300 em >=1600, 340 em
// >=1900), que valia sempre; o `larguraDefault` do módulo só roda na carga, então sem esta queda
// o degrau ficava congelado na largura do primeiro carregamento. NÃO salva: a escolha da pessoa
// fica intacta.
export function reclamparLargura(): void {
  let salva: number | null = null;
  try {
    const raw = Number(localStorage.getItem(CHAVE_LARGURA));
    if (Number.isFinite(raw) && raw > 0) salva = raw;
  } catch {
    /* sem storage: usa o valor atual */
  }
  ctxPanel.largura = clampLargura(salva ?? larguraDefault());
}
