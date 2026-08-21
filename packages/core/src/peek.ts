// ESPIADA (peek) do quadro: num quadro que mostra TODAS as máquinas, abrir o card de uma delas NÃO
// muda em qual servidor você está — ao fechar, o ativo volta pro de antes.
//
// Por que existe como função pura (e não solta dentro do $effect do App): esta regra já foi APAGADA
// uma vez por um refactor (62ee600 reverteu o fd79dda sem citar), sob a tese "o ativo é sempre função
// da rota". A tese é falsa — ela só vale nas rotas que CARREGAM serverId (#/chat/<s>/<n>,
// #/board/<s>/<n>); #/board puro, #/, #/costs e #/archive não carregam, e sem o restore o ativo ficava
// no servidor do último card espiado PRA SEMPRE (a sidebar seguia mostrando o antigo — selectServer
// não notifica —, então "+ nova sessão" nascia na máquina errada). Um comentário não impede a terceira
// deleção; um teste que fica vermelho impede. Aqui a regra é executável (ver peek.test.ts).

export type PeekMemo = {
  prev: string | null;      // servidor a restaurar ao fechar a espiada (null = nada a desfazer)
  peeking: boolean;         // a rota ANTERIOR já era a espiada?
  navigated: boolean;       // já houve rota REAL antes desta? ('loading'/'login' não contam)
};

export const initialPeek: PeekMemo = { prev: null, peeking: false, navigated: false };

/**
 * Um passo da máquina de espiada. Puro: não lê nem escreve o servidor ativo — recebe o `activeId` e
 * devolve o `restore` (id pra quem o chamador deve voltar, ou null). O chamador só roda isto em rota
 * REAL ('loading'/'login' ficam de fora, senão o boot contaria como navegação).
 *
 * @param peekServer servidor do card espiado (#/board/<serverId>/<nome>), ou null em qualquer outra rota
 */
export function peekStep(
  memo: PeekMemo,
  routeName: string,
  peekServer: string | null,
  activeId: string | null,
): { memo: PeekMemo; restore: string | null } {
  let prev = memo.prev;
  let restore: string | null = null;

  if (peekServer) {
    // Captura o ativo de ANTES uma vez só: re-run da mesma rota (ou trocar de card B->C direto) não
    // recaptura, senão o "de onde vim" viraria o card anterior. Deep-link FRIO (a 1ª rota da aba já é
    // a espiada) não captura: não houve de-onde-voltar, o ativo já é o do card.
    if (!memo.peeking) {
      const before = memo.navigated ? activeId : null;
      prev = before && before !== peekServer ? before : null;   // já era o ativo -> nada a restaurar
    }
  } else if (memo.peeking) {
    // Saiu da espiada. #/chat/<server>/<nome> PROMOVE (o Chat que vai montar é o do card) -> não
    // restaura. Toda outra saída (#/board, #/, #/costs, #/archive) volta pro servidor de antes.
    if (routeName !== 'chat' && prev) restore = prev;
    prev = null;
  }

  return { memo: { prev, peeking: !!peekServer, navigated: true }, restore };
}
