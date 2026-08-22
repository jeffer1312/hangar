import { create } from 'zustand';
import { openSessionsStream, aggregateSessions, sweepHidden } from '@hangar/core';
import type { SessionInfo, Server, AggSession } from '@hangar/core';
import { sortSessions } from '@hangar/core';
import type { Slot, ServerBucket, Aggregate } from '@hangar/core';
import { useServers } from './servers';

// Store vivo da lista de sessões — porte de frontend/src/lib/sessionsStore.svelte.ts
// para zustand. Um EventSource por servidor, refcount compartilhado, agregação via
// aggregateSessions (pura) e ordenação via sortSessions.
//
// Watchdog: o adapter mobile/src/net/sse.ts JÁ tem watchdog de 25s (fecha + onerror type
// timeout quando 25s sem evento; ping a cada ~10s rearma via wrap). Duplicar o relógio aqui
// fecharia o MESMO stream duas vezes e dispararia reconexão dupla. Este store CONFIA no
// adapter: não cria timer próprio de inatividade; apenas trata o onerror (inclui timeout do
// adapter) com backoff e rearma via evento 'ping' para que o wrap do adapter re-arme.

export interface SessionsState {
  rows: AggSession[];
  byServer: ServerBucket[];
  // conveniência para o que a Task descreve como Record<serverId, SessionInfo[]>
  byServerRecord: Record<string, AggSession[]>;
  loading: boolean;
  servers: Server[];
  retain: () => () => void;
  release: () => void;
  order: () => AggSession[];
  reconnect: () => void;
  refreshServers: () => void;
  markDeleting: (serverId: string, name: string) => void;
  unmarkDeleting: (serverId: string, name: string) => void;
}

// internas — fora do set() para não virar proxy
const slots = new Map<string, Slot>();
const streams = new Map<string, ReturnType<typeof openSessionsStream>>();
const retryDelays = new Map<string, number>();
const retryTimers = new Map<string, ReturnType<typeof setTimeout>>();
let hidden = new Set<string>();
let serversCache: Server[] = [];
let refs = 0;
let unsubServers: (() => void) | null = null;
let aggCache: Aggregate = { rows: [], byServer: [], loading: false };

const RETRY_MIN_MS = 5_000;
const RETRY_MAX_MS = 60_000;

function scheduleRetry(id: string, get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  const delay = retryDelays.get(id) ?? RETRY_MIN_MS;
  retryDelays.set(id, Math.min(delay * 2, RETRY_MAX_MS));
  clearTimeout(retryTimers.get(id));
  retryTimers.set(
    id,
    setTimeout(() => {
      retryTimers.delete(id);
      if (refs > 0 && serversCache.some((s) => s.id === id)) {
        connect(serversCache, get, set);
      }
    }, delay),
  );
}

function recompute(set: (p: Partial<SessionsState>) => void) {
  hidden = sweepHidden(hidden, slots);
  aggCache = aggregateSessions(serversCache, slots, hidden);
  const byServerRecord: Record<string, AggSession[]> = {};
  for (const b of aggCache.byServer) byServerRecord[b.server.id] = b.sessions;
  set({
    rows: aggCache.rows,
    byServer: aggCache.byServer,
    byServerRecord,
    loading: aggCache.loading,
    servers: serversCache,
  });
}

function connect(list: Server[], get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  // fecha streams de servidores removidos
  for (const [id, es] of streams) {
    if (!list.some((s) => s.id === id)) {
      es.close();
      streams.delete(id);
      slots.delete(id);
      clearTimeout(retryTimers.get(id));
      retryTimers.delete(id);
      retryDelays.delete(id);
    }
  }
  for (const s of list) {
    if (streams.has(s.id)) continue;
    if (retryTimers.has(s.id)) continue;
    const es = openSessionsStream(s);
    // ping mantém o watchdog do adapter vivo (wrap rearma). Sem listener,
    // ping não rearma e o adapter fecharia stream saudável.
    es.addEventListener('ping', () => {});
    es.addEventListener('sessions', (e) => {
      retryDelays.delete(s.id);
      try {
        slots.set(s.id, { sessions: JSON.parse(e.data) as SessionInfo[], error: null });
      } catch {
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
      }
      recompute(set);
    });
    es.addEventListener('list_error', () => {
      // resposta viva, só list falhou — mantém última lista boa, marca erro distinto
      // mas também consideramos como sinal de vida para não derrubar por watchdog
      // (o adapter já rearmou via wrap)
      slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
      recompute(set);
    });
    es.onerror = () => {
      slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
      recompute(set);
      es.close();
      streams.delete(s.id);
      scheduleRetry(s.id, get, set);
    };
    streams.set(s.id, es);
  }
  recompute(set);
}

function start(get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  serversCache = useServers.getState().servers.slice();
  connect(serversCache, get, set);
  // observa mudanças de servidores (useServers é zustand)
  unsubServers = useServers.subscribe((state) => {
    const next = (state as unknown as { servers: Server[] }).servers;
    const igual =
      next.length === serversCache.length &&
      next.every((s, i) => s.id === serversCache[i]?.id && s.baseUrl === serversCache[i]?.baseUrl && s.token === serversCache[i]?.token);
    if (igual) return;
    serversCache = next.slice();
    if (refs > 0) connect(serversCache, get, set);
    else recompute(set);
  });
}

function stop(set: (p: Partial<SessionsState>) => void) {
  unsubServers?.();
  unsubServers = null;
  for (const t of retryTimers.values()) clearTimeout(t);
  retryTimers.clear();
  retryDelays.clear();
  for (const es of streams.values()) es.close();
  streams.clear();
  slots.clear();
  // hidden mantém? Não — limpa ao parar para próximo retain começar limpo
  hidden = new Set<string>();
  recompute(set);
}

export const useSessions = create<SessionsState>((set, get) => ({
  rows: [],
  byServer: [],
  byServerRecord: {},
  loading: false,
  servers: [],
  retain() {
    if (++refs === 1) start(get, set);
    // retorna release para uso como cleanup de useEffect
    return () => get().release();
  },
  release() {
    if (refs > 0 && --refs === 0) stop(set);
  },
  order() {
    return sortSessions(get().rows);
  },
  reconnect() {
    if (refs === 0) return;
    for (const es of streams.values()) es.close();
    streams.clear();
    // limpa timers de retry para forçar reconexão imediata
    for (const t of retryTimers.values()) clearTimeout(t);
    retryTimers.clear();
    retryDelays.clear();
    connect(serversCache, get, set);
  },
  refreshServers() {
    serversCache = useServers.getState().servers.slice();
    if (refs > 0) connect(serversCache, get, set);
    else recompute(set);
  },
  markDeleting(serverId: string, name: string) {
    hidden.add(`${serverId}::${name}`);
    recompute(set);
  },
  unmarkDeleting(serverId: string, name: string) {
    hidden.delete(`${serverId}::${name}`);
    recompute(set);
  },
}));

// expõe reset apenas para testes (isso não vai para produção)
export function _resetSessionsForTests() {
  for (const t of retryTimers.values()) clearTimeout(t);
  retryTimers.clear();
  retryDelays.clear();
  for (const es of streams.values()) es.close();
  streams.clear();
  slots.clear();
  hidden = new Set<string>();
  serversCache = [];
  aggCache = { rows: [], byServer: [], loading: false };
  refs = 0;
  unsubServers?.();
  unsubServers = null;
  useSessions.setState({ rows: [], byServer: [], byServerRecord: {}, loading: false, servers: [] });
}
