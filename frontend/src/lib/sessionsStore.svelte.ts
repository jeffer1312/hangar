// Store ÚNICO da agregação SSE multi-servidor (substitui as 3 cópias de slots/recompute/connect —
// docs/polish-backlog.md § Structural debt). Singleton com refcount: Sidebar + Board/Canvas montados
// ao mesmo tempo compartilham 1 EventSource por servidor (limite ~6 SSE/host do navegador).
// Estratégia de stream = a do Board (a mais robusta das cópias): try/catch no parse + onServersChanged.
// Nota (architect): nas trocas board↔canvas↔chat do desktop o refcount nunca toca 0 porque a Sidebar
// fica montada o tempo todo segurando 1 retain — não há fecha-e-reabre de streams. Se um dia nenhum
// consumidor ficar permanentemente montado, considerar um keep-alive com delay no release.
import * as m from '../paraglide/messages';
import type { EventSourceLike } from '@hangar/core';
import { openSessionsStream } from '@hangar/core';
import { listServers, onServersChanged, type Server } from './auth';
import { navPelaLista } from './navPelaLista';
import { podarNavMortos } from './navegadorPanel.svelte';
import { aggregateSessions, sweepHidden, type Slot, type Aggregate } from '@hangar/core';

function createSessionsStore() {
  let servers = $state<Server[]>([]);
  // $state.raw: agg é SUBSTITUÍDO inteiro a cada recompute e nunca mutado — o proxy profundo do
  // $state só custava, e embrulhar as rows em proxy quebrava a identidade que o memo do
  // aggregateSessions preserva (rows de servidor que não emitiu = mesmo objeto -> keyed each das
  // views não re-renderiza os cards dos outros servidores).
  let agg = $state.raw<Aggregate>({ rows: [], byServer: [], loading: false });
  const slots = new Map<string, Slot>();
  const streams = new Map<string, EventSourceLike>();
  // Watchdog por stream (mesmo padrão do Chat): o backend emite `ping` a cada ~10s no stream de
  // lista justamente pra isto — suspend/VPN flap deixa a conexão MEIO-ABERTA sem onerror e as 4
  // views congelavam em silêncio até um reconnect manual. Sem sinal por 25s -> fecha e reabre.
  const watchdogs = new Map<string, ReturnType<typeof setTimeout>>();
  const WATCHDOG_MS = 25_000;
  // Prazo só do PRIMEIRO quadro (ver o comentário no connect). 10s e não 3s porque a medição de
  // 06/09/2026 achou 1,8s de p90 no caminho do celular quando o túnel está perdendo pacote —
  // apertar demais esconderia da lista um servidor que está no ar, e isso é pior que mostrar um
  // morto por mais alguns segundos.
  const PRIMEIRO_QUADRO_MS = 10_000;
  const primeiros = new Map<string, ReturnType<typeof setTimeout>>();
  // ms até o primeiro quadro de cada servidor — ver o comentário no `chegou` do connect().
  // `$state.raw` porque o Map é SUBSTITUÍDO inteiro a cada medição (mesma escolha do `agg` acima):
  // sem ser estado reativo, a reatribuição não chegaria em quem lê num `$derived`.
  let latencias = $state.raw(new Map<string, number>());
  // Backoff por servidor OFFLINE: o auto-retry do EventSource martela a cada ~3s pra sempre —
  // num tablet com 2+ servidores desligados isso é rádio/bateria à toa. Falhou -> fecha o stream
  // e re-tenta com espera crescente (5s -> 60s); qualquer frame bom zera a espera.
  const RETRY_MIN_MS = 5_000;
  const RETRY_MAX_MS = 60_000;
  const retryDelays = new Map<string, number>();
  const retryTimers = new Map<string, ReturnType<typeof setTimeout>>();
  // Agenda a re-tentativa de UM servidor com backoff. Usado pelo onerror E pelo watchdog — o
  // watchdog reconectando na hora deixava servidor PENDURADO (tailscale pra nó morto não recusa,
  // trava o socket) ciclando 25s/25s pra sempre e afogando os sockets do servidor bom no iOS.
  function scheduleRetry(id: string) {
    const delay = retryDelays.get(id) ?? RETRY_MIN_MS;
    retryDelays.set(id, Math.min(delay * 2, RETRY_MAX_MS));
    clearTimeout(retryTimers.get(id));
    retryTimers.set(id, setTimeout(() => {
      retryTimers.delete(id);
      if (refs > 0 && servers.some((x) => x.id === id)) connect(servers);
    }, delay));
  }
  let refs = 0;
  let offChanged: (() => void) | null = null;
  // Exclusão otimista: chaves `serverId::name` escondidas da lista enquanto o delete está em voo.
  // A faxina roda a cada recompute — quando o SSE confirma o sumiço, a marca sai sozinha.
  let hidden = new Set<string>();

  function recompute() {
    hidden = sweepHidden(hidden, slots);
    agg = aggregateSessions(servers, slots, hidden);
    const vivos = new Map<string, Map<string, string | null>>();
    for (const [id, slot] of slots) {
      if (slot.sessions && !slot.error) vivos.set(id, new Map(slot.sessions.map((s) => [s.name, s.jsonl ?? null])));
    }
    podarNavMortos(vivos);
  }

  // Reconcilia streams com a lista: fecha o que sumiu, abre o que entrou, mantém o resto.
  function connect(list: Server[]) {
    for (const [id, es] of streams) {
      if (!list.some((s) => s.id === id)) {
        es.close(); streams.delete(id); slots.delete(id);
        clearTimeout(watchdogs.get(id)); watchdogs.delete(id);
        clearTimeout(primeiros.get(id)); primeiros.delete(id);
        clearTimeout(retryTimers.get(id)); retryTimers.delete(id); retryDelays.delete(id);
        if (latencias.has(id)) { latencias = new Map(latencias); latencias.delete(id); }
      }
    }
    for (const s of list) {
      if (streams.has(s.id)) continue;
      const es = openSessionsStream(s);
      const arm = () => {
        clearTimeout(watchdogs.get(s.id));
        watchdogs.set(s.id, setTimeout(() => {
          // es.close() num stream já fechado é noop; connect() reabre só este servidor (os outros
          // seguem em streams). O arm() do stream novo substitui este timer no mesmo id.
          es.close();
          streams.delete(s.id);
          watchdogs.delete(s.id);
          cancelarPrazo();   // o stream acabou; NÃO mede — silêncio não é latência
          // Mesmo tratamento do onerror: o slot que motivou o watchdog está potencialmente velho —
          // marca offline (mantendo a última lista boa) em vez de segui-lo servindo como bom.
          slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
          recompute();
          scheduleRetry(s.id);
        }, WATCHDOG_MS));
      };
      // Prazo do PRIMEIRO quadro, separado do watchdog. O slot só nasce quando chega evento, então
      // até lá o servidor tem `error` nulo e passa por vivo em quem filtra offline — com o watchdog
      // de 25s isso era meio minuto oferecendo máquina desligada na folha de "Nova sessão". O
      // stream da lista manda `sessions` na conexão (medido em 5ms daqui), então silêncio longo
      // aqui é máquina fora do ar, não lentidão. Só MARCA: não fecha o stream nem mexe no retry,
      // pra um servidor lento que responda depois voltar sozinho no próximo evento.
      // Sem guarda de "o slot já existe": `reconnect()` (botão Atualizar) e `onVisibleKick` (celular
      // acordando) reabrem o stream MANTENDO o slot antigo, e ali a guarda fazia o prazo virar
      // no-op — o celular acordando é justamente quando isto precisa valer. O timer chegar a
      // disparar já prova o que interessa: nenhum quadro nesta conexão. Preserva a última lista boa
      // (mesmo tratamento do watchdog): "offline com dado velho" é diferente de "nunca respondeu",
      // e o banner de erro depende dessa distinção.
      const tPrimeiro = setTimeout(() => {
        if (primeiros.get(s.id) === tPrimeiro) primeiros.delete(s.id);
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        recompute();
      }, PRIMEIRO_QUADRO_MS);
      primeiros.set(s.id, tPrimeiro);
      // `delete` só se a entrada ainda for ESTE timer: um timer fantasma de tentativa anterior
      // apagaria do Map o timer da tentativa atual, e aí ninguém mais conseguiria cancelá-lo.
      // Custo da ROTA até esta máquina. A mesma máquina costuma estar cadastrada duas vezes, por
      // dois caminhos (Tailscale direto e o desvio pela VPS), e com as duas no ar não havia como
      // saber qual escolher: medido em 06/09/2026, 45ms contra 134ms pro mesmo backend. O tempo até
      // o primeiro quadro já inclui conexão e TLS, que é exatamente o que separa as rotas.
      // Mede a cada CONEXÃO, não só a primeira: ligar ou desligar a VPN troca a rota, e é
      // justamente aí que o número velho enganaria. `medido` é por conexão porque `chegou` também
      // roda nos pings seguintes, e ali o relógio já não mede abertura nenhuma.
      const abriuEm = performance.now();
      let medido = false;
      // CANCELAR o prazo e MEDIR são coisas separadas, e misturá-las inverte o sentido do número:
      // o watchdog e o `onerror` também precisam cancelar, e uma conexão RECUSADA na hora falha em
      // poucos ms — gravada como latência, a rota morta viraria "a mais rápida", em verde, que é
      // exatamente a escolha errada que este número existe pra evitar. Só quadro de verdade mede.
      const cancelarPrazo = () => {
        clearTimeout(tPrimeiro);
        if (primeiros.get(s.id) === tPrimeiro) primeiros.delete(s.id);
      };
      const chegou = () => {
        if (!medido) {
          medido = true;
          // Map NOVO, não `.set` no mesmo: quem lê isto num `$derived` acompanha a REFERÊNCIA —
          // mutar em lugar não avisa ninguém e o número nunca apareceria na tela.
          latencias = new Map(latencias).set(s.id, Math.round(performance.now() - abriuEm));
          recompute();
        }
        cancelarPrazo();
      };
      arm();
      // O ping é prova de vida (cancela o prazo), mas NÃO mede: ele só sai de 8 em 8 segundos, e
      // com o refresher do backend frio ele é o PRIMEIRO evento a chegar — a "latência da rota"
      // viraria ~8000ms de espera do servidor. Quem mede é o quadro de dados.
      es.addEventListener('ping', cancelarPrazo);
      es.addEventListener('sessions', chegou);
      es.addEventListener('ping', arm);
      es.addEventListener('sessions', (e) => {
        arm();
        retryDelays.delete(s.id);   // sinal de vida: proximo erro recomeca do backoff minimo
        try {
          slots.set(s.id, { sessions: JSON.parse(e.data), error: null });
        } catch {
          // Frame malformado: sem isto o throw sobe no dispatch do EventSource e o slot congela em
          // silêncio (onerror não dispara pra erro de parse). Mantém a última lista boa e avisa.
          slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        }
        recompute();
      });
      // O agente abriu o navegador embutido de uma sessão (possivelmente fora da tela) — ver
      // navPelaLista. Vai pelo stream da lista porque é o único que o desktop mantém sempre aberto.
      es.addEventListener('nav', (e) => {
        arm();
        void navPelaLista(s, (e as MessageEvent).data);
      });
      // Refresher do backend falhou (achado do hunter): sem isto, lista vazia por erro interno era
      // indistinguível de zero sessões. Mantém a última lista boa; o erro aparece distinto de offline.
      es.addEventListener('list_error', () => {
        arm();   // conexão está viva — só o produtor de dados falhou
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: m.sessao_erro_servidor() });
        recompute();
      });
      es.onerror = () => {
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        recompute();
        // Assume o controle do retry (o nativo martela): fecha e reagenda com backoff.
        es.close();
        streams.delete(s.id);
        clearTimeout(watchdogs.get(s.id)); watchdogs.delete(s.id);
        cancelarPrazo();   // o stream falhou; NÃO mede — falhar rápido não é ser rápido
        scheduleRetry(s.id);
      };
      streams.set(s.id, es);
    }
    recompute();
  }

  // Wake do aparelho (iOS congela timers em background): zera o backoff e reconecta os caidos NA
  // HORA — sem isto, o retry agendado pre-sleep deixava a lista "offline" por ate 60s com rede boa.
  function onVisibleKick() {
    if (document.visibilityState !== 'visible' || refs === 0) return;
    retryDelays.clear();
    for (const t of retryTimers.values()) clearTimeout(t);
    retryTimers.clear();
    connect(servers);
  }

  function start() {
    servers = listServers();
    connect(servers);
    offChanged = onServersChanged(() => { servers = listServers(); connect(servers); });
    document.addEventListener('visibilitychange', onVisibleKick);
  }
  function stop() {
    offChanged?.();
    offChanged = null;
    document.removeEventListener('visibilitychange', onVisibleKick);
    // Timers primeiro: um watchdog disparando pós-stop reabriria streams com refs = 0.
    for (const t of watchdogs.values()) clearTimeout(t);
    watchdogs.clear();
    for (const t of primeiros.values()) clearTimeout(t);
    primeiros.clear();
    // A poda por servidor removido mora no laço do `connect()`, que compara com `streams` — e aqui
    // `streams` já foi esvaziado. Sem zerar, um servidor apagado enquanto ninguém segurava o store
    // voltaria exibindo a latência de outra época, que ninguém mais vai corrigir.
    latencias = new Map();
    for (const t of retryTimers.values()) clearTimeout(t);
    retryTimers.clear(); retryDelays.clear();
    for (const es of streams.values()) es.close();
    streams.clear();
    slots.clear();
    recompute();
  }

  return {
    get rows() { return agg.rows; },
    get byServer() { return agg.byServer; },
    get latencias() { return latencias; },
    get loading() { return agg.loading; },
    get servers() { return servers; },
    retain() { if (++refs === 1) start(); },
    // Guarda contra consumidor futuro desbalanceado: um release a mais deixaria refs negativo e o
    // singleton nunca mais reconectaria (nenhum retain voltaria a bater 1). Piso em 0.
    release() { if (refs > 0 && --refs === 0) stop(); },
    reconnect() {
      // Resgata streams meio-abertos sem recarregar a página (o "Atualizar" dos menus).
      // refs=0 => ninguém consome o store (ex: Configurações aberta sobre Archive/Costs, onde a
      // lista não está montada): reconectar abriria SSE órfão — não faz nada.
      if (refs === 0) return;
      for (const es of streams.values()) es.close();
      streams.clear();
      connect(servers);
    },
    refreshServers() {
      servers = listServers();
      // Mesma guarda: com refs=0 a lista nova fica pronta pro próximo retain sem abrir stream.
      if (refs > 0) connect(servers);
    },
    // Exclusão otimista: a view marca antes do await (linha some na hora) e desmarca no catch
    // (linha REAPARECE = rollback visual). No sucesso ninguém desmarca — a faxina do recompute
    // remove a marca quando o SSE re-emitir a lista sem a sessão.
    markDeleting(serverId: string, name: string) { hidden.add(`${serverId}::${name}`); recompute(); },
    unmarkDeleting(serverId: string, name: string) { hidden.delete(`${serverId}::${name}`); recompute(); },
  };
}

export const sessionsStore = createSessionsStore();
