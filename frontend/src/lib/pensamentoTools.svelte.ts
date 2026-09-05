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
// A REGRA (o que entra no bloco) mora no core; aqui fica só a preferência do navegador.
import { entraNoPensamento as entraNoPensamentoCore, PENSAMENTO_TOOLS } from '@hangar/core';
import type { PensamentoTools } from '@hangar/core';

const KEY = 'cp_pensamento_tools';

export type { PensamentoTools };

const VALIDOS = PENSAMENTO_TOOLS;

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

/** Esta chamada entra no bloco do pensamento, no modo atual? */
export function entraNoPensamento(nome?: string | null): boolean {
  return entraNoPensamentoCore(pref, nome);
}

export { ehBusca } from '@hangar/core';
