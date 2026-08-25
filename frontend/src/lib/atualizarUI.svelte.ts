// Se a caixa de atualização está aberta. Existe porque quem PEDE a abertura e quem DESENHA a caixa
// não são vizinhos: o botão da barra e o da tela Sobre (dentro do modal de Configurações, montado
// pelo App) precisam abrir uma caixa que vive no DesktopShell. Passar callback do App até lá
// atravessaria três componentes que não têm nada a ver com atualização.
//
// Não persiste em localStorage, ao contrário do `quotaBarra`: "estou com a caixa aberta" é estado
// do momento, e reabri-la sozinha ao recarregar a página seria intrometido.
let aberta = $state(false);

export const atualizarUI = {
  get aberta() { return aberta; },
  abrir() { aberta = true; },
  fechar() { aberta = false; },
};
