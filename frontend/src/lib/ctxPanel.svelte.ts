// Estado do painel de contexto (a coluna da direita, no desktop): aberto ou recolhido em trilho.
//
// Vive fora dos dois componentes porque DOIS lados precisam do mesmo valor: o painel, pra desenhar
// o trilho, e o Chat, pra devolver a largura ao texto (`--ctx-w`). Sem uma fonte única, um dos dois
// ficaria com a faixa errada — o clássico "recolheu mas o chat não cresceu".
//
// Persiste em localStorage: reabrir a tela deve respeitar a escolha, como já acontece com a barra
// da esquerda (`loadPin` no Sidebar).

const CHAVE = 'cp_ctx_recolhido';

function carregar(): boolean {
  try {
    return localStorage.getItem(CHAVE) === '1';
  } catch {
    return false; // navegador sem storage (modo privado/embutido): o padrão é aberto
  }
}

export const ctxPanel = $state({ recolhido: carregar() });

export function alternarCtxPanel(): void {
  ctxPanel.recolhido = !ctxPanel.recolhido;
  try {
    localStorage.setItem(CHAVE, ctxPanel.recolhido ? '1' : '0');
  } catch {
    /* sem storage: vale só nesta sessão */
  }
}

// Largura da faixa reservada pelo Chat. Aberto: a mesma de sempre. Recolhido: só a aba da borda,
// o que devolve ~230px de leitura ao chat.
export const LARGURA_ABERTO = 264;
export const LARGURA_TRILHO = 34;
