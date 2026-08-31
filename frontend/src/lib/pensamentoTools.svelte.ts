// O que a chamada de ferramenta feita NO MEIO do raciocínio faz na conversa: fica escondida dentro
// do bloco recolhido do pensamento, ou continua como card à vista.
//
// Preferência do APP (localStorage), não do servidor — é escolha de quem olha a tela, e o mesmo
// servidor é lido de aparelhos diferentes. O interruptor irmão (`showThinkingSummaries`, em
// Servidor → Avançado) é outra coisa: aquele decide se o resumo do raciocínio EXISTE.
//
// Por que três valores e não um liga/desliga: medido nos transcripts desta máquina, 89,6% das
// chamadas caem entre dois pensamentos, e a maioria é Bash (1646), Edit (918) e Read (327). Em
// `tudo`, uma sessão de código inteira desaparece atrás de uma linha — o que é ótimo pra ler uma
// pesquisa e péssimo pra acompanhar trabalho. `busca` é o meio termo e o padrão.
//
// Mesmo padrão do taskRows/toolLook: chave no localStorage + $state, reage na hora, sem reload.
const KEY = 'cp_pensamento_tools';

/** `nada` = só o texto do pensamento entra no bloco; `busca` = + WebSearch/WebFetch/ToolSearch;
 *  `tudo` = + toda ferramenta que rodou entre dois pensamentos. */
export type PensamentoTools = 'nada' | 'busca' | 'tudo';

const VALIDOS: PensamentoTools[] = ['nada', 'busca', 'tudo'];

function carregar(): PensamentoTools {
  try {
    const v = localStorage.getItem(KEY) as PensamentoTools | null;
    // Valor desconhecido cai no padrão em vez de virar um quarto modo: o valor pode ter sido
    // escrito por uma versão futura (ou à mão), e um `if` que não casa com nenhum ramo deixaria a
    // conversa sem bloco nenhum.
    return v && VALIDOS.includes(v) ? v : 'busca';
  } catch {
    return 'busca';
  }
}

let pref = $state<PensamentoTools>(carregar());

export const pensamentoTools = {
  get pref() { return pref; },
  set pref(v: PensamentoTools) {
    pref = v;
    try {
      if (v === 'busca') localStorage.removeItem(KEY);   // padrão não ocupa espaço
      else localStorage.setItem(KEY, v);
    } catch { /* modo privado: vale pela sessão */ }
  },
};

const BUSCA = new Set(['WebSearch', 'WebFetch']);
// O ToolSearch é o carregador das outras ferramentas ("select:WebSearch,WebFetch"). Ele entra
// junto da busca porque, caindo entre o pensamento e a busca, fecharia o bloco no meio — e aí as
// primeiras buscas do turno ficavam de fora enquanto as seguintes entravam.
const CARREGADOR = 'ToolSearch';

// TaskCreate/TaskUpdate NUNCA entram, nem no modo "tudo": com a lista de tarefas ligada, essas
// duas chamadas são TROCADAS pela cápsula viva de tarefas no MessageList — e a troca acontece
// depois desta checagem. Engolidas aqui, a cápsula sumia da tela inteira e o que restava era uma
// linha crua "TaskCreate" dentro do bloco, sem checklist e sem progresso.
const TAREFA = new Set(['TaskCreate', 'TaskUpdate']);

/** Esta chamada entra no bloco do pensamento, no modo atual? */
export function entraNoPensamento(nome?: string | null): boolean {
  if (pref === 'nada') return false;
  if (nome && TAREFA.has(nome)) return false;
  if (pref === 'tudo') return true;
  return !!nome && (BUSCA.has(nome) || nome === CARREGADOR);
}

/** É busca? (o rótulo do bloco conta busca quando só há busca lá dentro) */
export function ehBusca(nome?: string | null): boolean {
  return !!nome && (BUSCA.has(nome) || nome === CARREGADOR);
}
