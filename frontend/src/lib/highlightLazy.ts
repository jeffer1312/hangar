// Fachada que ADIA o Shiki. Todo componente entra por aqui; ninguem importa `lib/highlight` de
// forma ESTATICA (import dinamico direto, como o do gitStore, continua valendo — nao puxa o pedaco).
//
// Por que a regra e essa: `highlight.ts` puxa shiki/core, o motor de regex em JS e os dois temas —
// medido em 28/08/2026 no mapa do bundle, ~148KB dos 1719KB do chunk de entrada. Basta UM import
// estatico dele num componente do caminho critico pra tudo isso ir junto pro chunk de entrada e
// ANULAR o `await import('./highlight')` que o gitStore ja fazia. Era exatamente o caso: o
// `AssistantBubble` importava estatico, entao toda conversa — inclusive as sem um bloco de codigo —
// baixava e parseava o realce de sintaxe inteiro.
//
// As tres funcoes ja eram assincronas, entao a fachada nao muda nada pra quem chama.
import type { DiffRow, DiffToken } from './highlight';

export type { DiffKind, DiffRow, DiffToken } from './highlight';

export async function highlightDiff(diffText: string, path: string): Promise<DiffRow[]> {
  return (await import('./highlight')).highlightDiff(diffText, path);
}

export async function highlightCodeLines(lines: string[], path: string): Promise<DiffToken[][] | null> {
  return (await import('./highlight')).highlightCodeLines(lines, path);
}

// O SELETOR e o mesmo do `highlightCodeBlocks`, repetido aqui de proposito: o early-return dele mora
// DENTRO do modulo, entao so vale depois de baixar os 205KB. O `AssistantBubble` chama isto a cada
// versao de CADA mensagem — sem o guarda, uma conversa sem um unico bloco de codigo baixava o Shiki
// inteiro (medido no navegador em 28/08/2026: zero `<pre>` na tela e o chunk pedido mesmo assim).
// Aqui so respondemos "existe bloco?"; qual deles ainda falta colorir continua sendo la dentro.
export async function highlightCodeBlocks(root: HTMLElement): Promise<void> {
  if (!root.querySelector('pre code[class^="language-"]')) return;
  // Os dois chamadores disparam com `void` (e um efeito de render): uma rejeicao aqui viraria
  // "Uncaught (in promise)" cru e o bloco ficaria sem cor sem nenhuma explicacao. Falha de INFRA
  // avisa — mesma disciplina do `getCore()` dentro do `highlight.ts`. Deploy que trocou o hash e
  // o caso comum, e quem cuida dele e o `vite:preloadError` do main.ts, que recarrega a pagina.
  try {
    return await (await import('./highlight')).highlightCodeBlocks(root);
  } catch (err) {
    console.warn('[hl] pedaco do realce nao carregou', err);
  }
}
