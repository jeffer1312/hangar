// Teleporta o nó pro <body>: ancestral com transform/filter/backdrop-filter cria containing block
// pra position:fixed, e o overlay ficaria preso na caixa do pai. Ver BottomSheet.svelte:170-178.
export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return { destroy() { node.remove(); } };
}
