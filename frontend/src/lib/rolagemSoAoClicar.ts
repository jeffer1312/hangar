// Bloco rolável dentro do chat só captura a roda do mouse DEPOIS de um clique nele.
//
// O problema: rolando a conversa, se o ponteiro passa por cima de um bloco com scroll próprio
// (saída de comando, diff), a roda passa a rolar o bloco em vez da conversa — e como esses blocos
// usam `overscroll-behavior: contain` (pra chegar no fim deles não sacudir o chat), a rolagem fica
// PRESA ali: só sai tirando o mouse de cima. Atravessar a conversa vira uma corrida de obstáculos.
//
// A regra: nasce inerte (sem scroll próprio, a roda atravessa pra conversa); um clique liga; tirar o
// ponteiro de cima desliga de novo.
//
// SÓ em ponteiro fino. No celular não há hover nem clique separado do toque, e arrastar dentro do
// bloco é o gesto natural — travar ali quebraria o que funciona. O recorte é feito no CSS
// (`@media (pointer: fine)`), então em tela de toque a classe existe e não faz nada.
export function rolagemSoAoClicar(node: HTMLElement) {
  const TRAVADA = 'rolagem-travada';
  node.classList.add(TRAVADA);

  const ligar = () => node.classList.remove(TRAVADA);
  const desligar = () => node.classList.add(TRAVADA);

  // O clique que LIGA a rolagem não pode subir: o card de ferramenta inteiro (ToolCard) tem um
  // onclick que colapsa, então clicar no diff pra poder rolar fechava o card no mesmo gesto — e a
  // rolagem que acabou de ser ligada morria junto. Só em ponteiro fino, o mesmo recorte do CSS:
  // no toque a rolagem já é nativa e o toque no resultado segue fechando o card como antes.
  const aoClicar = (e: MouseEvent) => {
    if (matchMedia('(pointer: fine)').matches) e.stopPropagation();
    ligar();
  };

  node.addEventListener('click', aoClicar);
  // `mouseleave` e não `mouseout`: mouseout dispara ao passar entre filhos do próprio bloco e
  // desligaria a rolagem no meio do uso.
  node.addEventListener('mouseleave', desligar);
  // Teclado: quem chega por Tab também precisa poder rolar, e sair do foco desliga igual.
  node.addEventListener('focusin', ligar);
  node.addEventListener('focusout', desligar);

  return {
    destroy() {
      node.removeEventListener('click', aoClicar);
      node.removeEventListener('mouseleave', desligar);
      node.removeEventListener('focusin', ligar);
      node.removeEventListener('focusout', desligar);
    },
  };
}
