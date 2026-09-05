// O que a chamada de ferramenta feita NO MEIO do raciocínio faz na conversa: fica escondida dentro
// do bloco recolhido do pensamento, ou continua como card à vista. A preferência é de quem olha a
// tela (localStorage na PWA, MMKV no app), então ela entra como ARGUMENTO — aqui mora só a regra.

/** `nada` = só o texto do pensamento entra no bloco; `busca` = + WebSearch/WebFetch/ToolSearch;
 *  `tudo` = + toda ferramenta que rodou entre dois pensamentos. */
export type PensamentoTools = 'nada' | 'busca' | 'tudo';
export const PENSAMENTO_TOOLS: PensamentoTools[] = ['nada', 'busca', 'tudo'];

const BUSCA = new Set(['WebSearch', 'WebFetch']);
// O ToolSearch é o carregador das outras ferramentas ("select:WebSearch,WebFetch"). Ele entra
// junto da busca porque, caindo entre o pensamento e a busca, fecharia o bloco no meio — e aí as
// primeiras buscas do turno ficavam de fora enquanto as seguintes entravam.
const CARREGADOR = 'ToolSearch';
// TaskCreate/TaskUpdate NUNCA entram, nem no modo "tudo": elas são TROCADAS pela cápsula viva de
// tarefas na lista, e a troca acontece depois desta checagem. Engolidas aqui, a cápsula sumia.
const TAREFA = new Set(['TaskCreate', 'TaskUpdate']);

/** É busca? (o rótulo do bloco conta busca quando só há busca lá dentro) */
export function ehBusca(nome?: string | null): boolean {
  return !!nome && (BUSCA.has(nome) || nome === CARREGADOR);
}

/** Esta chamada entra no bloco do pensamento, no modo dado? */
export function entraNoPensamento(pref: PensamentoTools, nome?: string | null): boolean {
  if (pref === 'nada') return false;
  if (nome && TAREFA.has(nome)) return false;
  if (pref === 'tudo') return true;
  return ehBusca(nome);
}

/** Linha fechada do bloco: a primeira frase do pensamento, quando ela é curta. */
export function resumoPensamento(texto: string): string {
  const limpo = texto.replace(/\s+/g, ' ').trim();
  const fim = limpo.search(/[.!?](\s|$)/);
  return fim > 0 && fim < 140 ? limpo.slice(0, fim + 1) : limpo;
}
