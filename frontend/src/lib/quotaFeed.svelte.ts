// Feed compartilhado de /api/cotas — UMA busca e UM relógio pro app inteiro: a faixa do rodapé
// (QuotaStrip) e a pílula do topo (QuotaPill) leem o mesmo número no mesmo instante, em vez de
// cada uma pagar o próprio poll e eventualmente mostrarem leituras diferentes por 59 segundos.
// O backend já guarda a leitura por 5 min, então o ciclo de 60s quase nunca toca a rede.
//
// Contagem de referência (retain/release): o intervalo só existe com consumidor montado. O
// endpoint é do servidor ATIVO — o consumidor chama setServidor quando a sessão/servidor muda e a
// próxima leitura vai pra máquina certa (geração descarta resposta em voo do servidor anterior).
import { listarCotas, type CotaConta } from './contaEstado';

let contas = $state<CotaConta[]>([]);
// Relógio local: a idade da leitura e a contagem até o reset andam sem bater na rede.
let agora = $state(Date.now() / 1000);

let servidor = '';
let refs = 0;
let timer: ReturnType<typeof setInterval> | null = null;
let geracao = 0;

async function carregar(): Promise<void> {
  const g = ++geracao;
  try {
    // `null` explícito, mesma decisão escrita do QuotaStrip: o servidor ATIVO.
    const lista = await listarCotas(null);
    if (g !== geracao) return;
    contas = lista;
  } catch {
    // Falha de rede não apaga leitura boa: o que está na tela envelhece sozinho (o `agora` sobe e
    // a conta vira `velha` com a idade ao lado). Zerar fazia a cota sumir num 500 de um segundo.
  }
}

export const quotaFeed = {
  get contas() { return contas; },
  get agora() { return agora; },
  /** Servidor ativo mudou → relê já (a geração protege a resposta velha que voltar depois). As
   *  contas do servidor anterior NÃO ficam na tela enquanto a nova leitura não chega: mostrar as
   *  cotas de outra máquina como se fossem deste é pior que a pílula vazia por um segundo. */
  setServidor(k: string) {
    if (k === servidor) return;
    servidor = k;
    contas = [];
    void carregar();
  },
  /** Força releitura (botão "Atualizar" do popover da pílula). */
  atualizar() { void carregar(); },
  retain() {
    refs += 1;
    // O relógio reanima no mount: se ele ficasse no valor da importação do módulo, a idade de uma
    // leitura chegaria CURTA demais (2h virariam 1h59m e a frase floria errado).
    agora = Date.now() / 1000;
    if (!timer) {
      void carregar();
      timer = setInterval(() => {
        agora = Date.now() / 1000;
        void carregar();
      }, 60_000);
    }
  },
  release() {
    refs = Math.max(0, refs - 1);
    if (refs === 0 && timer) {
      clearInterval(timer);
      timer = null;
    }
  },
  /** Testes: zera o singleton entre mounts (estado de um teste não pode vazar pro próximo). */
  resetParaTeste() {
    contas = [];
    agora = Date.now() / 1000;
    servidor = '';
    refs = 0;
    geracao += 1;
    if (timer) { clearInterval(timer); timer = null; }
  },
};
