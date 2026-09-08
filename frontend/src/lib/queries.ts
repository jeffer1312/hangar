// Queries do app sobre o TanStack Query. É AQUI que mora a decisão de chave/TTL/invalidação — a
// tela só chama `createQuery(() => orqPolitica())`. Trocar de biblioteca no futuro é reescrever
// este arquivo, não caçar `fetch` espalhado pelas telas.
//
// Sem `QueryClientProvider`: o `createQuery` aceita o client como 2º argumento (accessor), então
// um singleton de módulo evita envolver o App inteiro só pra isso. Quem monta a tela passa
// `clienteQuery` junto — ver `OrquestracaoSheet.svelte`.
import { QueryClient, queryOptions } from '@tanstack/svelte-query';
import { getActiveId, type Server } from './auth';
import {
  fetchCostsForServer, getArchive, getEngines, getEnginesForServer, getOrqDetalheForServer,
  getOrqGrupo, getOrqPolitica,
  type ArchiveFolder, type EnginesResponse,
} from '@hangar/core';
import { listarCredenciais } from './credenciais';
import type { OrqGrupo, OrqPolitica } from '@hangar/core';
import type { ChatEvent, CostReport, OrqExecucao } from '@hangar/core';

export const clienteQuery = new QueryClient({
  defaultOptions: {
    queries: {
      // `gcTime` é o que faz o stale-while-revalidate valer entre ABERTURAS: sem ele (padrão 5min)
      // o cache some junto com o último consumidor, e sair da tela e voltar paga o spinner de novo.
      gcTime: 30 * 60_000,
      // Sem repetição automática. Cada fetch daqui já tem seu próprio teto de tempo, e o retry do
      // TanStack é incondicional — repetiria um 401 ou 404 que nunca vai mudar, dobrando a espera
      // antes de o erro aparecer. As telas têm caminho de erro próprio (aviso, "tentar de novo",
      // fallback por texto): erro rápido e visível vale mais aqui do que uma segunda tentativa
      // calada. Medido: com retry, o `.catch` que carrega os modelos no CreateSessionSheet
      // demorava o bastante para a tela ficar sem lista.
      retry: false,
    },
  },
});

// `getOrqPolitica`/`getOrqGrupo` batem no servidor ATIVO (apiFetch → getBaseUrl), que muda debaixo
// da tela. Sem o id na chave, trocar de máquina serviria o cache da anterior — o mesmo defeito que
// o `serverIdentidade` de ContasSettings e a época do `_catCache` já evitam nos seus caminhos.
const idAtivo = () => getActiveId() ?? '-';

export const orqPolitica = () => queryOptions({
  queryKey: ['orq', 'politica', idAtivo()],
  queryFn: (): Promise<OrqPolitica> => getOrqPolitica(),
  staleTime: 60_000,
});

export const orqGrupo = (name: string) => queryOptions({
  queryKey: ['orq', 'grupo', idAtivo(), name],
  queryFn: (): Promise<OrqGrupo> => getOrqGrupo(name),
  staleTime: 30_000,
});

// Detalhe de uma execução de orquestração. Execução TERMINADA (`fim` preenchido) nunca muda mais —
// `staleTime: Infinity` porque rebuscá-la é gastar disco do servidor pra confirmar o que já se sabe.
// A viva muda a cada rodada, então vale poucos segundos.
// `terminada` entra na CHAVE, não só no staleTime: o `fetchQuery` compara a idade do dado com o
// staleTime daquela chamada, então uma execução lida enquanto viva e depois reaberta como terminada
// caía em `Infinity` por cima do snapshot do MEIO da execução — e nunca mais buscava. O detalhe
// ficava sem a última rodada e sem o veredito, parecendo o final. Chaves separadas fazem a
// transição viva→terminada nascer sem dado, que é uma leitura a mais e a resposta certa.
export const orqDetalhe = (s: Server, id: string, terminada: boolean) => queryOptions({
  queryKey: ['orq', 'detalhe', s.id, id, terminada],
  queryFn: (): Promise<OrqExecucao> => getOrqDetalheForServer(s, id),
  staleTime: terminada ? Infinity : 5_000,
});

// Contas de uma máquina. A chave leva o id do alvo, NÃO o `serverIdentidade` — token em chave de
// cache impediria persistir isto em disco depois. Trocar o token do MESMO servidor (consertar uma
// credencial) continua recarregando: quem cuida disso é o efeito de identidade da própria tela,
// que já existia e chama refetch.
export const credenciais = (alvo: Server | null) => queryOptions({
  // `null` = servidor ATIVO, e ele precisa virar o id REAL aqui. Com o literal 'ativo' na chave,
  // trocar o servidor ativo e reabrir Contas dentro do staleTime servia as contas e as cotas da
  // máquina ANTERIOR sob o nome da nova, sem spinner e sem erro — e `serverIdentidade(null)`
  // também devolve constante, então nem o efeito de identidade da tela percebia a troca.
  queryKey: ['credenciais', alvo?.id ?? idAtivo()],
  queryFn: () => listarCredenciais(alvo),
  staleTime: 60_000,
});

// O engines.json da máquina — o que dá "modelo e opções" à chave de API na tela Contas. Mesma
// regra de chave da `credenciais`: id REAL do alvo, nunca um literal para "ativo".
export const motores = (alvo: Server | null) => queryOptions({
  queryKey: ['motores', alvo?.id ?? idAtivo()],
  queryFn: (): Promise<EnginesResponse> => (alvo ? getEnginesForServer(alvo) : getEngines()),
  staleTime: 60_000,
});

// Custo de UMA máquina num período. É a leitura mais cara do app — medida em 12,6s com o cache do
// backend frio (0,28s quente). Cacheia por (máquina, período) e não o merge: o merge é código puro
// e barato, e é a TROCA DE PERÍODO que o usuário faz o tempo todo — 7d → 30d → 7d pagava tudo de
// novo na volta. 5 min porque o período corrente inclui HOJE, e o custo de hoje ainda sobe.
export const custos = (s: Server, periodo: string) => queryOptions({
  queryKey: ['custos', s.id, periodo],
  queryFn: (): Promise<Partial<CostReport>> => fetchCostsForServer(s, periodo),
  staleTime: 5 * 60_000,
});

// Pastas do Arquivo: lista de projetos com conversa arquivada. Muda quando alguém arquiva algo,
// o que é raro perto da frequência com que a tela é aberta e fechada.
export const arquivo = () => queryOptions({
  queryKey: ['arquivo', idAtivo()],
  queryFn: (): Promise<ArchiveFolder[]> => getArchive(),
  staleTime: 60_000,
});

/**
 * Aquece as duas queries do painel de Orquestração ao ENTRAR na sessão, pro primeiro toque no
 * botão já achar o dado pronto (mesmo papel do prefetch de `getModelOptions` no Composer).
 * `prefetchQuery` respeita o staleTime e engole o erro: aquecer é otimização, nunca falha visível.
 */
export function prefetchOrq(name: string): void {
  void clienteQuery.prefetchQuery(orqPolitica());
  void clienteQuery.prefetchQuery(orqGrupo(name));
}

/**
 * Aquece a lista de contas quando o ponteiro passa por um caminho que leva a Configurações. Ali a
 * leitura das cotas é a parte cara (medida em ~2,5s com o cache do backend frio), e o hover dá o
 * adiantamento que a entrada da sessão não dá — ninguém "entra em Configurações" antes de clicar.
 * `prefetchQuery` respeita o staleTime, então passar o mouse dez vezes não são dez requisições.
 */
export function prefetchContas(alvo: Server | null): void {
  void clienteQuery.prefetchQuery(credenciais(alvo));
  void clienteQuery.prefetchQuery(motores(alvo));
}

// ── Cauda do chat entre ABERTURAS ────────────────────────────────────────────────────────────
// Entrar numa sessão baixava tudo de novo, toda vez: no celular a rota #/chat desmonta o Chat, e
// com ele morre `events`. Medido em 06/09/2026 na sessão `pr-junior` (transcript de 31,9 MB), pelo
// caminho que o iPhone percorre: 313 KB de cauda por ENTRADA, mesmo voltando dez segundos depois
// sem nada ter mudado.
//
// Guarda aqui, e não num mapa de módulo próprio, porque o `gcTime` de 30min do cliente já é
// exatamente o que se quer ("faz o stale-while-revalidate valer entre ABERTURAS", ver o comentário
// dele) e a decisão de chave/TTL mora neste arquivo. NÃO é uma query: o `events` do chat é um
// buffer VIVO — o SSE acrescenta, o dedup tira eco, `appendTail`/`prependOlder` costuram pontas —
// e isso continua no Chat. Aqui é só o depósito da última cauda conhecida.
//
// O `etag` é o que dispensa regra de invalidação. Já se tentou invalidar por evento ("depois que
// EU mando mensagem"), e aquilo não cobre a mesma sessão sendo escrita pelo terminal, por outro
// aparelho, ou continuando a trabalhar depois que você saiu. Com o validador, quem responde
// "mudou?" é o disco do servidor — e `/clear` (transcript outro) cai no mesmo caminho.
//
// Só eventos e validador. O `lastEventId` do stream JÁ morou aqui e foi tirado: é offset em BYTES
// e restaurá-lo fazia o SSE retomar à frente do que o cache continha. O stream faz o backfill dele
// sozinho; o dedup por id do Chat descarta o que já está na tela.
//
// O servidor vem por ARGUMENTO, não de `idAtivo()`: ao navegar entre chats de máquinas diferentes,
// `applyRouteServer` (App.svelte) chama `selectServer` ANTES de o Chat antigo desmontar — o
// `onDestroy` dele resolveria o servidor NOVO e gravaria a conversa de uma máquina sob a chave da
// outra. Com sessões de mesmo nome nas duas (o `hangar` local e o da VPS), a próxima abertura
// pintava a conversa errada antes de qualquer rede. Quem chama captura o id uma vez, na entrada.
export interface CaudaChat { eventos: ChatEvent[]; etag: string | null }

const chaveCauda = (servidor: string, name: string) => ['chat-cauda', servidor, name] as const;

export function lerCaudaChat(servidor: string, name: string): CaudaChat | undefined {
  return clienteQuery.getQueryData<CaudaChat>(chaveCauda(servidor, name));
}

export function guardarCaudaChat(servidor: string, name: string, cauda: CaudaChat): void {
  clienteQuery.setQueryData(chaveCauda(servidor, name), cauda);
}
