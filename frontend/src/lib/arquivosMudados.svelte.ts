import { getChangedFiles } from '@hangar/core';

export interface ArquivoMudado {
  path: string;
  added: number;
  total: number;
}

// As maiores mudanças do working tree, pro bloco "Mais alterados" do painel de contexto.
//
// Mora aqui, e não dentro do componente, por dois motivos. O DesktopSessionContext tem uma prop
// `state`, e o rune `$state` colide com ela. E o valor precisa SOBREVIVER ao recarregamento: com
// a promessa direto num `{#await}` a lista sumia e voltava a cada poll do git, piscando na tela.
export function criarArquivosMudados(quantos = 3) {
  let itens = $state<ArquivoMudado[]>([]);
  let ultima = '';
  let ultimaSessao = '';
  let geracao = 0;
  let emVoo = false;

  return {
    get itens() {
      return itens;
    },
    // `chave` identifica esta versão do repositório (sessão + contadores do git): mesma chave,
    // nenhuma chamada. Mudou, busca de novo — e o que já está na tela fica até a resposta chegar.
    //
    // Chave VAZIA não limpa nada: ela só quer dizer "agora não dá pra perguntar" (a listagem
    // ainda não trouxe os contadores, por exemplo). Limpar aqui matava a carga em voo e o bloco
    // nunca aparecia. Quem decide não mostrar é o componente, pelo estado do repositório.
    async carregar(sessionName: string, chave: string): Promise<void> {
      // Troca de sessão limpa na hora: segurar o valor só faz sentido entre duas leituras do
      // MESMO repositório. O painel não remonta ao trocar de sessão, então sem isto a sessão nova
      // abria mostrando os arquivos da anterior até a resposta dela chegar.
      if (sessionName !== ultimaSessao) {
        ultimaSessao = sessionName;
        ultima = '';
        itens = [];
        geracao++;   // descarta resposta da sessão antiga que ainda esteja em voo
      }
      if (!sessionName) {
        itens = [];
        ultima = '';
        return;
      }
      if (!chave || chave === ultima) return;
      ultima = chave;
      // Um pedido por vez: chave que muda com o pedido no ar só é buscada quando ele volta, e
      // sempre a mais recente. Sem isto, git lento no backend empilhava um pedido por mudança.
      if (!emVoo) await buscar();
    },
  };

  async function buscar(): Promise<void> {
    emVoo = true;
    let feita = '';
    try {
      while (ultima && ultima !== feita) {
        const chave = ultima;
        const minha = ++geracao;
        try {
          const r = await getChangedFiles(ultimaSessao);
          if (minha !== geracao) continue;   // trocou de sessão no meio: descarta
          itens = r.files
            .map((f) => ({ path: f.path, added: f.added ?? 0, total: (f.added ?? 0) + (f.removed ?? 0) }))
            .filter((f) => f.total > 0)
            .sort((a, b) => b.total - a.total)
            .slice(0, quantos);
        } catch {
          // Bloco acessório: o painel inteiro não pode cair porque o git não respondeu. O erro de
          // git aparece no painel de git, que é quem fala disso.
          if (minha === geracao) itens = [];
        } finally {
          feita = chave;
        }
      }
    } finally {
      emVoo = false;
    }
  }
}
