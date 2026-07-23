// Store ÚNICO da agregação SSE multi-servidor (substitui as 3 cópias de slots/recompute/connect —
// docs/polish-backlog.md § Structural debt). Singleton com refcount: Sidebar + Board/Canvas montados
// ao mesmo tempo compartilham 1 EventSource por servidor (limite ~6 SSE/host do navegador).
// Estratégia de stream = a do Board (a mais robusta das cópias): try/catch no parse + onServersChanged.
// Nota (architect): nas trocas board↔canvas↔chat do desktop o refcount nunca toca 0 porque a Sidebar
// fica montada o tempo todo segurando 1 retain — não há fecha-e-reabre de streams. Se um dia nenhum
// consumidor ficar permanentemente montado, considerar um keep-alive com delay no release.
import { openSessionsStream } from './api';
import { listServers, onServersChanged, type Server } from './auth';
import { aggregateSessions, sweepHidden, type Slot, type Aggregate } from './sessions';

function createSessionsStore() {
  let servers = $state<Server[]>([]);
  // $state.raw: agg é SUBSTITUÍDO inteiro a cada recompute e nunca mutado — o proxy profundo do
  // $state só custava, e embrulhar as rows em proxy quebrava a identidade que o memo do
  // aggregateSessions preserva (rows de servidor que não emitiu = mesmo objeto -> keyed each das
  // views não re-renderiza os cards dos outros servidores).
  let agg = $state.raw<Aggregate>({ rows: [], byServer: [], loading: false });
  const slots = new Map<string, Slot>();
  const streams = new Map<string, EventSource>();
  // Watchdog por stream (mesmo padrão do Chat): o backend emite `ping` a cada ~10s no stream de
  // lista justamente pra isto — suspend/VPN flap deixa a conexão MEIO-ABERTA sem onerror e as 4
  // views congelavam em silêncio até um reconnect manual. Sem sinal por 25s -> fecha e reabre.
  const watchdogs = new Map<string, ReturnType<typeof setTimeout>>();
  const WATCHDOG_MS = 25_000;
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
  }

  // Reconcilia streams com a lista: fecha o que sumiu, abre o que entrou, mantém o resto.
  function connect(list: Server[]) {
    for (const [id, es] of streams) {
      if (!list.some((s) => s.id === id)) {
        es.close(); streams.delete(id); slots.delete(id);
        clearTimeout(watchdogs.get(id)); watchdogs.delete(id);
        clearTimeout(retryTimers.get(id)); retryTimers.delete(id); retryDelays.delete(id);
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
          // Mesmo tratamento do onerror: o slot que motivou o watchdog está potencialmente velho —
          // marca offline (mantendo a última lista boa) em vez de segui-lo servindo como bom.
          slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
          recompute();
          scheduleRetry(s.id);
        }, WATCHDOG_MS));
      };
      arm();
      es.addEventListener('ping', arm);
      es.addEventListener('sessions', (e) => {
        arm();
        retryDelays.delete(s.id);   // sinal de vida: proximo erro recomeca do backoff minimo
        try {
          slots.set(s.id, { sessions: JSON.parse((e as MessageEvent).data), error: null });
        } catch {
          // Frame malformado: sem isto o throw sobe no dispatch do EventSource e o slot congela em
          // silêncio (onerror não dispara pra erro de parse). Mantém a última lista boa e avisa.
          slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        }
        recompute();
      });
      es.onerror = () => {
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        recompute();
        // Assume o controle do retry (o nativo martela): fecha e reagenda com backoff.
        es.close();
        streams.delete(s.id);
        clearTimeout(watchdogs.get(s.id)); watchdogs.delete(s.id);
        scheduleRetry(s.id);
      };
      streams.set(s.id, es);
    }
    recompute();
  }

  function start() {
    servers = listServers();
    connect(servers);
    offChanged = onServersChanged(() => { servers = listServers(); connect(servers); });
  }
  function stop() {
    offChanged?.();
    offChanged = null;
    // Timers primeiro: um watchdog disparando pós-stop reabriria streams com refs = 0.
    for (const t of watchdogs.values()) clearTimeout(t);
    watchdogs.clear();
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
    get loading() { return agg.loading; },
    get servers() { return servers; },
    retain() { if (++refs === 1) start(); },
    // Guarda contra consumidor futuro desbalanceado: um release a mais deixaria refs negativo e o
    // singleton nunca mais reconectaria (nenhum retain voltaria a bater 1). Piso em 0.
    release() { if (refs > 0 && --refs === 0) stop(); },
    reconnect() {
      // Resgata streams meio-abertos sem recarregar a página (o "Atualizar" dos menus).
      for (const es of streams.values()) es.close();
      streams.clear();
      connect(servers);
    },
    refreshServers() { servers = listServers(); connect(servers); },
    // Exclusão otimista: a view marca antes do await (linha some na hora) e desmarca no catch
    // (linha REAPARECE = rollback visual). No sucesso ninguém desmarca — a faxina do recompute
    // remove a marca quando o SSE re-emitir a lista sem a sessão.
    markDeleting(serverId: string, name: string) { hidden.add(`${serverId}::${name}`); recompute(); },
    unmarkDeleting(serverId: string, name: string) { hidden.delete(`${serverId}::${name}`); recompute(); },
  };
}

export const sessionsStore = createSessionsStore();
