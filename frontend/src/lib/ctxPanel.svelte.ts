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

export const ctxPanel = $state({
  recolhido: carregar(),
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

// Largura da faixa reservada pelo Chat. Aberto: a mesma de sempre. Recolhido: só a aba da borda,
// o que devolve ~230px de leitura ao chat.
export const LARGURA_ABERTO = 264;
export const LARGURA_TRILHO = 34;
