import { getConfig, patchConfig, type EstiloDitado, ehEstilo } from '@hangar/core';

// Estilo do ditado, vivendo perto do microfone e nao enterrado nas Configuracoes: a decisao "isto
// aqui e um comando rapido" ou "isto e um pedido inteiro" muda ANTES de falar, e ninguem abre modal
// pra isso. O valor persiste no servidor (runtime_config.ditado_estilo), entao o atalho do teclado
// (Ctrl+Espaco) ja grava no estilo escolhido sem precisar saber que ele existe — quem le a config e
// o backend, na hora de limpar.
//
// Store proprio, e nao o criarConfigServidor: aquele carrega ciclo de rascunho/salvar/desfazer pra
// uma tela inteira de campos. Aqui e UM campo que salva no clique.

const PADRAO: EstiloDitado = 'prosa';

let atual = $state<EstiloDitado>(PADRAO);
let carregado = false;
let carregando: Promise<void> | null = null;
// Contador de ESCRITAS, mesmo padrão de geração do criarConfigServidor. Existe porque GET e PATCH
// não têm ordem entre si e nada impede dois cliques seguidos:
//   (1) o GET da montagem responde DEPOIS de uma troca já persistida e pinta o valor velho por
//       cima — e como `carregado` virou true, nenhuma leitura futura corrige;
//   (2) de duas trocas sobrepostas, a PRIMEIRA falhando reverteria por cima da SEGUNDA, que deu
//       certo no servidor.
// Nos dois casos a tela mostraria um estilo que o servidor não tem, calada. Quem chegou depois
// manda: leitura só pinta se nenhuma escrita começou no meio, e reversão só vale pra escrita que
// ainda é a última.
let escritas = 0;

export const ditadoEstilo = {
  get valor(): EstiloDitado { return atual; },

  /** true quando `valor` ja veio do SERVIDOR (ou foi trocado por aqui). Existe porque o estilo
   *  agora viaja no /transcribe e VENCE a config do servidor: mandar o padrao chutado, antes do
   *  primeiro GET responder, seria o app sobrescrevendo a escolha da pessoa com um palpite. Quem
   *  nao esta pronto simplesmente nao manda estilo — e o backend le a config, como sempre fez. */
  get pronto(): boolean { return carregado; },

  /** Le do servidor DE NOVO, mesmo ja tendo lido. A pill mostrava valor velho por horas: o app le
   *  a config uma vez por carga de pagina, e uma troca feita noutra aba/aparelho nunca chegava —
   *  a tela dizia "So limpar" e o ditado voltava em briefing (visto ao vivo 21/08/2026). Chamado
   *  ao ABRIR o popover: e o instante em que a pessoa vai olhar a lista, e o custo e um GET. */
  revalidar(): Promise<void> {
    carregado = false;
    return this.carregar();
  },

  /** Le do servidor UMA vez. Chamadas concorrentes compartilham a mesma promessa — o Composer monta
   *  em varias telas (chat, peek do quadro) e cada uma chamaria isto ao mesmo tempo. */
  carregar(): Promise<void> {
    if (carregado) return Promise.resolve();
    if (carregando) return carregando;
    // try/catch em volta da CHAMADA, não só .catch() no resultado: getConfig pode estourar de
    // forma SÍNCRONA (rede indisponível no fetch, ou um módulo mockado sem esse export nos
    // testes), e aí o erro escapa do encadeamento de promessas e sobe como exceção não tratada
    // dentro do $effect que roda na montagem do Composer — ou seja, no caminho do microfone, que
    // é justamente o que esta função promete nunca derrubar.
    const escritasNoInicio = escritas;
    try {
      carregando = getConfig()
        .then((cfg) => {
          const v = cfg.campos?.ditado_estilo?.valor;
          // Só pinta se NENHUMA troca começou enquanto este GET estava em voo: o que ele traz é o
          // valor de antes dela, e o do usuário é mais novo que o do servidor lido.
          if (ehEstilo(v) && escritas === escritasNoInicio) atual = v;
          carregado = true;
        })
        // Falha de leitura NAO pode travar o microfone: fica o padrao, e o proximo clique no chip
        // tenta de novo (carregado segue false). O erro que importa — o de SALVAR — aparece na tela.
        .catch(() => {})
        .finally(() => { carregando = null; });
      return carregando;
    } catch {
      carregando = null;
      return Promise.resolve();
    }
  },

  /** Troca e persiste. Pinta na hora (otimista) e VOLTA ATRAS se o servidor recusar: mostrar
   *  "Briefing" selecionado enquanto o servidor guarda outra coisa e mentira na cara do usuario. */
  async trocar(novo: EstiloDitado): Promise<void> {
    const antes = atual;
    const minha = ++escritas;
    atual = novo;
    try {
      await patchConfig({ ditado_estilo: novo });
      carregado = true;
    } catch (e) {
      // Só reverte se esta ainda for a última troca. Se outra começou depois, ela é quem manda —
      // reverter aqui apagaria da tela um valor que já foi persistido com sucesso.
      if (escritas === minha) atual = antes;
      throw e;
    }
  },

  /** Só pros testes: o store é singleton com cache, então sem zerar um caso herda o `carregado` do
   *  anterior e as corridas ficam impossíveis de montar. */
  _zerarParaTeste(): void {
    atual = PADRAO;
    carregado = false;
    carregando = null;
    escritas = 0;
  },
};
