import { create } from 'zustand';
import { openSessionsStream, aggregateSessions, jsonlDaSessao, sweepHidden, registrarDiag, decidirRota, discoverRemoteRoute, esquecerRota, rotaDecidida, baseOf, UNCONFIRMED_BASE, probeServerResponse, comTeto, placeExternalRows, listExternalPairs, attachGuest, createExternalPairSync } from '@hangar/core';
import type { SessionInfo, Server, AggSession, ExternalPair } from '@hangar/core';
import { sortSessions, definirArmazem, definirProtegido, definirSemPrazo, registrarFalha, registrarSucesso, retryAfterMs, estaDesligado, respondeuRecentemente, retentarAgora, esquecerServidor, onServerRecovered, _limparEsfriamentoParaTestes } from '@hangar/core';
import type { Slot, ServerBucket, Aggregate } from '@hangar/core';
import { useServers } from './servers';
import * as m from '../paraglide/messages';

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
  setForeground: (active: boolean) => void;
  refreshServers: () => void;
  markDeleting: (serverId: string, name: string) => void;
  unmarkDeleting: (serverId: string, name: string) => void;
}

// internas — fora do set() para não virar proxy
const slots = new Map<string, Slot>();
const streams = new Map<string, ReturnType<typeof openSessionsStream>>();
type ConnectionAttempt = { server: Server; generation: number; abort: AbortController; routing: boolean };
const connecting = new Map<string, ConnectionAttempt>();
const routeDeadlines = new WeakMap<Promise<void>, number>();
const failures = new Map<string, number>();
const retryAt = new Map<string, number>();
// Falha da lista não é necessariamente queda de rede; só lista SSE válida encerra este recuo.
const httpFailures = new Map<string, number>();
const httpRetryAt = new Map<string, number>();
const lastFailureHttp = new Map<string, boolean>();
const HTTP_RETRY_DELAYS_MS = [2000, 5000, 30000, 60000, 120000, 240000, 300000, 600000, 1800000];
const refused = new Map<string, number>();
const retryTimers = new Map<string, ReturnType<typeof setTimeout>>();
let hidden = new Map<string, string | null>();
let serversCache: Server[] = [];
let refs = 0;
let foreground = true;
let generation = 0;
let unsubServers: (() => void) | null = null;
let unsubRecovered: (() => void) | null = null;
let activeId: string | null = null;
let initialKickPending = true;
let aggCache: Aggregate = { rows: [], byServer: [], loading: false };
// O aviso de tela vem de fora: o store não carrega o toast (e com ele o nativo) nos testes.
let pairNotice: (text: string) => void = () => {};
export function setPairNotice(fn: (text: string) => void) { pairNotice = fn; }
let machineDiscovery: (server: Server) => void = () => {};
export function setMachineDiscovery(fn: (server: Server) => void) { machineDiscovery = fn; }
let storeGet: (() => SessionsState) | null = null;
let storeSet: ((p: Partial<SessionsState>) => void) | null = null;

function configureCooldown() {
  definirProtegido((id) => {
    const attempt = connecting.get(id);
    return !foreground || useServers.getState().activeId === id || (attempt !== undefined && !isCurrent(attempt));
  });
  definirSemPrazo((id) => useServers.getState().activeId === id);
}
configureCooldown();

function retryDelay(id: string) {
  const networkDelay = useServers.getState().activeId === id
    ? Math.max(0, (retryAt.get(id) ?? 0) - Date.now())
    : retryAfterMs(id);
  return Math.max(networkDelay, (httpRetryAt.get(id) ?? 0) - Date.now());
}

function refusedError(status: number) {
  return status === 403 ? m.lista_acesso_recusado() : `${m.sessao_erro_servidor()} (${status})`;
}

function hasServer(server: Server) {
  return serversCache.some((s) => s.id === server.id && s.baseUrl === server.baseUrl && s.token === server.token);
}

function isCurrent(attempt: ConnectionAttempt) {
  return foreground && refs > 0 && attempt.generation === generation && !attempt.abort.signal.aborted && hasServer(attempt.server);
}

function disconnect(id: string, abortPending = true) {
  const stream = streams.get(id);
  streams.delete(id);
  stream?.close();
  clearTimeout(retryTimers.get(id));
  retryTimers.delete(id);
  const attempt = connecting.get(id);
  if (!abortPending && attempt && !attempt.routing) connecting.delete(id);
  if (abortPending) attempt?.abort.abort();
  esquecerRota(id);
}

function pause() {
  generation++;
  for (const id of new Set([...streams.keys(), ...retryTimers.keys(), ...connecting.keys()])) disconnect(id, false);
}

function scheduleRetry(id: string, get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  if (!foreground || refs === 0 || retryTimers.has(id)) return;
  const delay = retryDelay(id);
  const servidor = serversCache.find((s) => s.id === id);
  if (servidor) registrarDiag({ evento: 'lista.retentativa', tela: 'lista', espera_ms: delay }, servidor.baseUrl);
  clearTimeout(retryTimers.get(id));
  retryTimers.set(
    id,
    setTimeout(() => {
      retryTimers.delete(id);
      if (foreground && refs > 0 && serversCache.some((s) => s.id === id)) {
        connect(serversCache, get, set, id);
      }
    }, delay),
  );
}

// Cada publicação traz o JSON do servidor inteiro: sem reaproveitar o que não mudou, toda linha
// ganha objeto novo e o memo do SessionRow não segura nada.
const sessionJson = new WeakMap<SessionInfo, string>();
function reuseUnchanged(prev: SessionInfo[] | null | undefined, next: SessionInfo[]): SessionInfo[] {
  const byName = new Map((prev ?? []).map((p) => [p.name, p]));
  let same = !!prev && prev.length === next.length;
  const out = next.map((item, i) => {
    const json = JSON.stringify(item);
    const old = byName.get(item.name);
    if (old && sessionJson.get(old) === json) {
      if (prev![i] !== old) same = false;
      return old;
    }
    same = false;
    sessionJson.set(item, json);
    return item;
  });
  return same ? prev! : out;
}

let stableRows = new Map<string, AggSession>();
function sameRow(a: AggSession, b: AggSession): boolean {
  const ka = Object.keys(a) as (keyof AggSession)[];
  return ka.length === Object.keys(b).length && ka.every((k) => a[k] === b[k]);
}
function stabilizeRows(agg: { rows: AggSession[]; byServer: ServerBucket[] }) {
  const next = new Map<string, AggSession>();
  const seen = new Map<AggSession, AggSession>();
  const keep = (row: AggSession) => {
    let out = seen.get(row);
    if (out) return out;
    const key = `${row.serverId}::${row.name}::${row.outside ? 'outside' : ''}`;
    const prev = stableRows.get(key);
    out = prev && sameRow(prev, row) ? prev : row;
    seen.set(row, out);
    next.set(key, out);
    return out;
  };
  const byServer = agg.byServer.map((b) => ({ ...b, sessions: b.sessions.map(keep) }));
  const rows = agg.rows.map(keep);
  stableRows = next;
  return { rows, byServer };
}

function recompute(set: (p: Partial<SessionsState>) => void) {
  hidden = sweepHidden(hidden, slots);
  const agg = aggregateSessions(serversCache, slots, hidden);
  aggCache = { ...stabilizeRows(placeExternalRows(agg)), loading: agg.loading };
  const byServerRecord: Record<string, AggSession[]> = {};
  for (const b of aggCache.byServer) byServerRecord[b.server.id] = b.sessions;
  set({
    rows: aggCache.rows,
    byServer: aggCache.byServer,
    byServerRecord,
    loading: aggCache.loading,
    servers: serversCache,
  });
  pairsMaybeChanged();
}

// Os pares nas linhas próprias dizem quando reler `/api/external-pairs`; o laço é o do core.
function pairsMaybeChanged() {
  if (!foreground || refs === 0 || !storeGet || !storeSet) return;
  void pairSync.refresh(serversCache.filter((s) => !s.disabled), (id) => slots.get(id)?.sessions);
}

function newPairSync() {
  return createExternalPairSync({
    read: listExternalPairs,
    apply: (all) => useServers.getState().applyPairs(all),
    attach: (entry, token) => attachGuest(entry, token).catch((error: unknown) => {
      registrarDiag({ evento: 'lista.falhou', nivel: 'erro', tela: 'lista', codigo: 'par_attach',
        detalhe: error instanceof Error ? error.name : 'erro' }, entry.baseUrl);
      throw error;
    }),
    onLeft: (p) => pairNotice(m.pair_left_notice({ dono: p.owner })),
    onAttached: (entry) => {
      disconnect(entry.id);
      if (storeGet && storeSet) connect(serversCache, storeGet, storeSet);
    },
  });
}
let pairSync = newPairSync();

/** "Sair"/"Desfazer" pelo próprio menu: o par sumir em seguida não é a outra pessoa saindo. */
export function markLeavingPair(alias: string, session: string): () => void {
  return pairSync.markLeaving(alias, session);
}

/** Pares cuja ponta é esta sessão, pela última leitura: sair do grupo os desfaz para todos. */
export function anchoredPairs(serverId: string, name: string): ExternalPair[] {
  return pairSync.anchored(serverId, name);
}

function connect(list: Server[], get: () => SessionsState, set: (p: Partial<SessionsState>) => void, onlyId?: string) {
  for (const id of slots.keys()) if (!list.some((s) => s.id === id)) slots.delete(id);
  if (!foreground || refs === 0) { recompute(set); return; }
  for (const s of list) {
    if (onlyId !== undefined && s.id !== onlyId) continue;
    // Recusa não muda por insistir; só a pessoa ou uma credencial nova liberam a conexão.
    const status = refused.get(s.id);
    if (status !== undefined) {
      slots.set(s.id, { sessions: null, error: refusedError(status) });
      continue;
    }
    if (s.inviteEnded) continue;
    if (streams.has(s.id) || retryTimers.has(s.id) || connecting.has(s.id)) continue;
    if (retryDelay(s.id) > 0) {
      slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: lastFailureHttp.get(s.id) ? m.sessao_erro_servidor() : 'offline' });
      scheduleRetry(s.id, get, set);
      continue;
    }
    if (estaDesligado(s.id)) slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
    const attempt = { server: { ...s }, generation, abort: new AbortController(), routing: true };
    connecting.set(s.id, attempt);
    void synchronize(attempt, get, set);
  }
  recompute(set);
}

async function synchronize(attempt: ConnectionAttempt, get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  const s = attempt.server;
  let countedFailure = false;
  let countedHttpFailure = false;
  let discovered = false;
  const failed = (alreadyCounted = false) => {
    if (countedFailure) return;
    countedFailure = true;
    const count = Math.min((failures.get(s.id) ?? 0) + 1, 6);
    failures.set(s.id, count);
    retryAt.set(s.id, Date.now() + Math.min(30_000, 1000 * 2 ** (count - 1)));
    // O probe já contou a falha REST; SSE sem resposta é a mesma tentativa.
    if (!alreadyCounted) registrarFalha(s.id);
  };
  const answered = () => {
    countedFailure = false;
    failures.delete(s.id);
    retryAt.delete(s.id);
    registrarSucesso(s.id);
  };
  const httpFailed = (retryAfter?: string | null) => {
    if (countedHttpFailure) return;
    countedHttpFailure = true;
    const count = Math.min((httpFailures.get(s.id) ?? 0) + 1, HTTP_RETRY_DELAYS_MS.length);
    httpFailures.set(s.id, count);
    const value = retryAfter?.trim() ?? '';
    const requested = /^\d+$/.test(value) ? Number(value) * 1000 : Date.parse(value) - Date.now();
    const delay = Math.min(HTTP_RETRY_DELAYS_MS[HTTP_RETRY_DELAYS_MS.length - 1],
      Math.max(HTTP_RETRY_DELAYS_MS[count - 1], Number.isNaN(requested) ? 0 : requested));
    httpRetryAt.set(s.id, Date.now() + delay);
  };
  const refreshRemote = () => {
    const current = useServers.getState().servers.find((server) => server.id === s.id);
    if (current && !current.invite && current.lan?.id && !current.remote) {
      void discoverRemoteRoute(current).then((remote) => {
        if (remote && isCurrent(attempt)) useServers.getState().rememberRemote(current.id, remote, current);
      });
    }
  };
  const receivedList = () => {
    countedHttpFailure = false;
    httpFailures.delete(s.id);
    httpRetryAt.delete(s.id);
    lastFailureHttp.delete(s.id);
    answered();
    if (!discovered) {
      discovered = true;
      machineDiscovery(s);
    }
    refreshRemote();
  };
  const rejected = (status: number) => {
    if (s.invite && (status === 401 || status === 410)) {
      useServers.getState().markEnded(s.id);
      slots.delete(s.id);
      recompute(set);
      return true;
    }
    if (!s.invite && status === 401) {
      useServers.getState().markInvalid(s.id);
      return true;
    }
    if (status === 403) {
      refused.set(s.id, status);
      slots.set(s.id, { sessions: null, error: refusedError(status) });
      recompute(set);
      return true;
    }
    return false;
  };
  try {
    if (!rotaDecidida(s.id)) {
      const route = decidirRota(s);
      const deadline = routeDeadlines.get(route) ?? Date.now() + 6000;
      routeDeadlines.set(route, deadline);
      let timer: ReturnType<typeof setTimeout> | undefined;
      try {
        // O fetch pode ignorar o abort; o prazo corta a espera, não a decisão compartilhada.
        const timedOut = await Promise.race([
          route.then(() => {
            if (!hasServer(s)) esquecerRota(s.id);
            return false;
          }),
          new Promise<boolean>((resolve) => { timer = setTimeout(() => resolve(true), Math.max(0, deadline - Date.now())); }),
        ]);
        if (timedOut && isCurrent(attempt)) {
          esquecerRota(s.id);
          registrarDiag({ evento: 'rota.falhou', nivel: 'aviso', codigo: 'route_timeout' }, s.baseUrl);
          if (baseOf(s) === UNCONFIRMED_BASE) throw new Error('server_identity_unconfirmed');
        }
      } finally {
        clearTimeout(timer);
      }
    }
    attempt.routing = false;
    if (!isCurrent(attempt)) return;
    let httpResponse = false;
    try {
      const response = await probeServerResponse(s, '/api/sessions', { signal: comTeto(attempt.abort.signal, 4000) });
      if (!isCurrent(attempt)) return;
      httpResponse = true;
      answered();
      if (rejected(response.status)) return;
      if (!response.ok) {
        httpFailed(response.status === 429 ? response.headers?.get('Retry-After') : null);
        throw new Error(String(response.status));
      }
      const sessions = await response.json() as SessionInfo[];
      if (!isCurrent(attempt)) return;
      if (!Array.isArray(sessions)) throw new Error('invalid_sessions');
      slots.set(s.id, { sessions: reuseUnchanged(slots.get(s.id)?.sessions, sessions), error: null });
      recompute(set);
    } catch (error) {
      if (!isCurrent(attempt)) return;
      lastFailureHttp.set(s.id, httpResponse);
      registrarDiag({ evento: 'lista.falhou', nivel: 'erro', tela: 'lista', codigo: 'sincronia',
        detalhe: error instanceof Error ? error.name : 'erro' }, s.baseUrl);
      if (!httpResponse) failed(true);
      slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: httpResponse ? m.sessao_erro_servidor() : 'offline' });
      recompute(set);
      if (countedHttpFailure) {
        esquecerRota(s.id);
        scheduleRetry(s.id, get, set);
        return;
      }
    }
    if (!isCurrent(attempt)) return;
    const es = openSessionsStream(s);
    const active = () => isCurrent(attempt) && streams.get(s.id) === es;
    let falhaDados = false;
    // ping mantém o watchdog do adapter vivo (wrap rearma). Sem listener,
    // ping não rearma e o adapter fecharia stream saudável.
    es.addEventListener('ping', () => {
      if (!active()) return;
      answered();
      if (discovered) refreshRemote();
    });
    es.addEventListener('sessions', (e) => {
      if (!active()) return;
      answered();
      try {
        const sessions: unknown = JSON.parse(e.data);
        if (!Array.isArray(sessions)) throw new Error('invalid_sessions');
        receivedList();
        slots.set(s.id, { sessions: reuseUnchanged(slots.get(s.id)?.sessions, sessions as SessionInfo[]), error: null });
        if (falhaDados) registrarDiag({ evento: 'lista.voltou', tela: 'lista' }, s.baseUrl);
        falhaDados = false;
      } catch {
        falhaDados = true;
        registrarDiag({ evento: 'lista.falhou', nivel: 'erro', tela: 'lista', codigo: 'json_invalido' }, s.baseUrl);
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: m.sessao_erro_servidor() });
      }
      recompute(set);
    });
    es.addEventListener('list_error', () => {
      if (!active()) return;
      answered();
      falhaDados = true;
      registrarDiag({ evento: 'lista.falhou', nivel: 'erro', tela: 'lista', codigo: 'produtor_falhou' }, s.baseUrl);
      // Resposta viva, só o produtor falhou. Com linhas na tela a falha é só do diário (um erro
      // anterior sai: o servidor respondeu); sem lista, a pessoa não vê sessão nenhuma e o erro fica.
      const slot = slots.get(s.id);
      const sessions = slot?.sessions ?? null;
      if (sessions?.length && !slot?.error) return;
      slots.set(s.id, { sessions, error: sessions?.length ? null : m.sessao_erro_servidor() });
      recompute(set);
    });
    es.onerror = (event) => {
      if (!active()) return;
      const status = (event as { xhrStatus?: number } | null)?.xhrStatus ?? 0;
      const http = Number.isInteger(status) && status >= 100 && status <= 599;
      lastFailureHttp.set(s.id, http);
      if (http) answered();
      else failed();
      streams.delete(s.id);
      es.close();
      if (http && rejected(status)) return;
      httpFailed();
      slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: http ? m.sessao_erro_servidor() : 'offline' });
      recompute(set);
      esquecerRota(s.id);
      scheduleRetry(s.id, get, set);
    };
    streams.set(s.id, es);
  } catch (error) {
    if (!isCurrent(attempt)) return;
    lastFailureHttp.set(s.id, false);
    registrarDiag({ evento: 'lista.falhou', nivel: 'erro', tela: 'lista', codigo: 'conexao',
      detalhe: error instanceof Error ? error.name : 'erro' }, s.baseUrl);
    failed();
    httpFailed();
    slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
    recompute(set);
    esquecerRota(s.id);
    scheduleRetry(s.id, get, set);
  } finally {
    if (connecting.get(s.id) === attempt) {
      connecting.delete(s.id);
      if (!isCurrent(attempt)) {
        // A decisão compartilhada de rota não aceita abort; só a geração atual pode usá-la.
        esquecerRota(s.id);
        if (foreground && refs > 0) connect(serversCache, get, set);
      }
    }
  }
}

function refresh(get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  const state = useServers.getState();
  const previous = serversCache;
  serversCache = state.servers.slice();
  if (initialKickPending && state.ready && foreground && refs > 0) {
    initialKickPending = false;
    for (const s of serversCache) if (respondeuRecentemente(s.id)) {
      retentarAgora(s.id);
      refused.delete(s.id);
    }
  }
  for (const s of previous) {
    if (!hasServer(s)) {
      disconnect(s.id);
      esquecerServidor(s.id);
      failures.delete(s.id);
      retryAt.delete(s.id);
      httpFailures.delete(s.id);
      httpRetryAt.delete(s.id);
      lastFailureHttp.delete(s.id);
      refused.delete(s.id);
      slots.delete(s.id);
    } else if (serversCache.find((next) => next.id === s.id)?.inviteEnded) {
      disconnect(s.id);
      slots.delete(s.id);
    }
  }
  if (activeId !== state.activeId) {
    activeId = state.activeId;
    if (activeId && !streams.has(activeId)) {
      clearTimeout(retryTimers.get(activeId));
      retryTimers.delete(activeId);
      retryAt.delete(activeId);
    }
  }
  connect(serversCache, get, set);
}

function start(get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  storeGet = get;
  storeSet = set;
  unsubRecovered = onServerRecovered((id) => {
    refused.delete(id);
    failures.delete(id);
    retryAt.delete(id);
    clearTimeout(retryTimers.get(id));
    retryTimers.delete(id);
    if (!streams.has(id) && !connecting.has(id)) connect(serversCache, get, set, id);
  });
  unsubServers = useServers.subscribe((state, previousState) => {
    const same = state.servers.length === serversCache.length && state.servers.every((s, index) => {
      const old = serversCache[index];
      return s.id === old?.id && s.baseUrl === old.baseUrl && s.token === old.token && s.inviteEnded === old.inviteEnded;
    });
    if (same && state.activeId === activeId && state.ready === previousState.ready) { serversCache = state.servers.slice(); return; }
    refresh(get, set);
  });
  refresh(get, set);
}

function stop(set: (p: Partial<SessionsState>) => void) {
  unsubServers?.();
  unsubServers = null;
  unsubRecovered?.();
  unsubRecovered = null;
  pause();
  slots.clear();
  for (const [id, status] of refused) slots.set(id, { sessions: null, error: refusedError(status) });
  // hidden mantém? Não — limpa ao parar para próximo retain começar limpo
  hidden = new Map<string, string | null>();
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
    if (refs === 0 || !foreground) return;
    for (const s of serversCache) {
      if (connecting.has(s.id) || (streams.has(s.id) && !slots.get(s.id)?.error)) continue;
      disconnect(s.id);
      retentarAgora(s.id);
      retryAt.delete(s.id);
      httpFailures.delete(s.id);
      httpRetryAt.delete(s.id);
      refused.delete(s.id);
      connect(serversCache, get, set, s.id);
    }
  },
  setForeground(active) {
    if (active === foreground) return;
    foreground = active;
    if (!active) pause();
    else {
      if (refs > 0 && useServers.getState().ready) initialKickPending = false;
      for (const s of serversCache) {
        const wasRefused = refused.delete(s.id);
        httpRetryAt.delete(s.id);
        if (wasRefused || s.id === useServers.getState().activeId || respondeuRecentemente(s.id)) {
          retentarAgora(s.id);
          retryAt.delete(s.id);
        }
      }
      connect(serversCache, get, set);
    }
  },
  refreshServers() {
    if (foreground && refs > 0) refused.clear();
    refresh(get, set);
  },
  markDeleting(serverId: string, name: string) {
    hidden.set(`${serverId}::${name}`, jsonlDaSessao(slots, serverId, name));
    recompute(set);
  },
  unmarkDeleting(serverId: string, name: string) {
    hidden.delete(`${serverId}::${name}`);
    recompute(set);
  },
}));

// expõe reset apenas para testes (isso não vai para produção)
export function _resetSessionsForTests() {
  pause();
  connecting.clear();
  foreground = true;
  retryTimers.clear();
  failures.clear();
  retryAt.clear();
  httpFailures.clear();
  httpRetryAt.clear();
  lastFailureHttp.clear();
  refused.clear();
  streams.clear();
  slots.clear();
  hidden = new Map<string, string | null>();
  serversCache = [];
  aggCache = { rows: [], byServer: [], loading: false };
  stableRows = new Map();
  pairSync.reset();
  pairSync = newPairSync();
  storeGet = null;
  storeSet = null;
  refs = 0;
  activeId = null;
  initialKickPending = true;
  unsubServers?.();
  unsubServers = null;
  unsubRecovered?.();
  unsubRecovered = null;
  _limparEsfriamentoParaTestes();
  definirArmazem(null);
  configureCooldown();
  useSessions.setState({ rows: [], byServer: [], byServerRecord: {}, loading: false, servers: [] });
}
