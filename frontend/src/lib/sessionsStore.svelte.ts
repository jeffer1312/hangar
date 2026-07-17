// Store ÚNICO da agregação SSE multi-servidor (substitui as 3 cópias de slots/recompute/connect —
// docs/polish-backlog.md § Structural debt). Singleton com refcount: Sidebar + Board/Canvas montados
// ao mesmo tempo compartilham 1 EventSource por servidor (limite ~6 SSE/host do navegador).
// Estratégia de stream = a do Board (a mais robusta das cópias): try/catch no parse + onServersChanged.
// Nota (architect): nas trocas board↔canvas↔chat do desktop o refcount nunca toca 0 porque a Sidebar
// fica montada o tempo todo segurando 1 retain — não há fecha-e-reabre de streams. Se um dia nenhum
// consumidor ficar permanentemente montado, considerar um keep-alive com delay no release.
import { openSessionsStream } from './api';
import { listServers, onServersChanged, type Server } from './auth';
import { aggregateSessions, type Slot, type Aggregate } from './sessions';

function createSessionsStore() {
  let servers = $state<Server[]>([]);
  let agg = $state<Aggregate>({ rows: [], byServer: [], loading: false });
  const slots = new Map<string, Slot>();
  const streams = new Map<string, EventSource>();
  let refs = 0;
  let offChanged: (() => void) | null = null;

  function recompute() { agg = aggregateSessions(servers, slots); }

  // Reconcilia streams com a lista: fecha o que sumiu, abre o que entrou, mantém o resto.
  function connect(list: Server[]) {
    for (const [id, es] of streams) {
      if (!list.some((s) => s.id === id)) { es.close(); streams.delete(id); slots.delete(id); }
    }
    for (const s of list) {
      if (streams.has(s.id)) continue;
      const es = openSessionsStream(s);
      es.addEventListener('sessions', (e) => {
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
    release() { if (--refs === 0) stop(); },
    reconnect() {
      // Resgata streams meio-abertos sem recarregar a página (o "Atualizar" dos menus).
      for (const es of streams.values()) es.close();
      streams.clear();
      connect(servers);
    },
    refreshServers() { servers = listServers(); connect(servers); },
  };
}

export const sessionsStore = createSessionsStore();
