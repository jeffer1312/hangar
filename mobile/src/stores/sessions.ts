import { create } from 'zustand';
import { openSessionsStream, deviceIdentityReady, aggregateSessions, parseWhatsAppOutboxEvent, jsonlDaSessao, sweepHidden, registrarDiag, decidirRota, discoverConnectRoute, discoverRemoteRoute, upgradeToDevice, esquecerRota, rotaDecidida, baseOf, UNCONFIRMED_BASE, probeServerResponse, comTeto, parseTagsEvent, parseWorkReviewsEvent, parseIntegrationApprovalEvent, patchSlot, placeExternalRows, listExternalPairs, attachGuest, createExternalPairSync } from '@hangar/core';
import type { SessionInfo, Server, AggSession, ExternalPair, TagsEvent, VaultEvent, VaultRequest, IntegrationApprovalEvent, WhatsAppOutboxEvent } from '@hangar/core';
import { sortSessions, definirArmazem, definirProtegido, definirSemPrazo, isServerParked, parkServer, registrarFalha, registrarSucesso, retryAfterMs, estaDesligado, startOutage, endOutage, respondeuRecentemente, retentarAgora, esquecerServidor, onServerRecovered, _limparEsfriamentoParaTestes } from '@hangar/core';
import type { Slot, ServerBucket, Aggregate } from '@hangar/core';
import { useServers } from './servers';
import { checkPeer } from '../features/machines/api';
import { parseNoticeIgnores } from '../features/notices/notices';
import { clearWorkReviewSummaries, setWorkReviewSummaries } from '../features/workReview/summaries';
import * as m from '../paraglide/messages';
import { appStartedAt } from '../net/appStart';

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
  /** Último evento `vault` de cada servidor; vale só enquanto o balde dele está sem erro. */
  vault: Record<string, VaultEvent>;
  /** Último evento `integrations_approvals` de cada servidor; limpo ao trocar token/desconectar. */
  integrationApprovals: Record<string, IntegrationApprovalEvent>;
  /** Último evento `whatsapp_outbox` de cada servidor (rascunhos pendentes); como o cofre, vale só com o balde sem erro. */
  whatsappOutbox: Record<string, WhatsAppOutboxEvent>;
  /** Etiquetas de máquina e contas de cada servidor (evento `tags`); o valor de cada servidor só muda quando a etiqueta dele muda. */
  tags: Record<string, TagsEvent>;
  /** Chaves de aviso da CLI do Claude ignoradas em cada servidor (evento `notice_ignores`). */
  noticeIgnores: Record<string, string[]>;
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
// Sem resposta boa o chaveiro deixa de valer; lista e etiquetas ficam.
// O início gravado da queda vale depois de reabrir: só uma lista boa (`endOutage`) o encerra.
const failedSlot = (id: string, error: string) => patchSlot(slots, id, { error, keyring: undefined, offlineSince: startOutage(id) });
const streams = new Map<string, ReturnType<typeof openSessionsStream>>();
const vaultRaw = new Map<string, string>();
const integrationApprovalsRaw = new Map<string, string>();
const whatsappOutboxRaw = new Map<string, string>();
const workReviewsRaw = new Map<string, string>();
const VAULT_STATES = new Set(['open', 'locked', 'absent', 'offline', 'key_lost']);
/** Campos guardados por máquina a partir de um evento da lista. */
type KeyedField = 'vault' | 'integrationApprovals' | 'whatsappOutbox';
type KeyedEvent<K extends KeyedField> = SessionsState[K] extends Record<string, infer E> ? E : never;

/** Evento `vault` torto devolve `null`; cartão sem id, interação ou tipo conhecido sai da lista. */
export function parseVaultEvent(data: string): VaultEvent | null {
  let raw: unknown;
  try { raw = JSON.parse(data); } catch { return null; }
  if (!raw || typeof raw !== 'object') return null;
  const { state, requests } = raw as { state?: unknown; requests?: unknown };
  if (state !== null && !VAULT_STATES.has(state as string)) return null;
  if (!Array.isArray(requests)) return null;
  const ok = requests.filter((r): r is VaultRequest => !!r && typeof r === 'object'
    && typeof (r as VaultRequest).id === 'string' && typeof (r as VaultRequest).interaction_id === 'string'
    && ((r as VaultRequest).kind === 'approve' || (r as VaultRequest).kind === 'entry'));
  return { state: state as VaultEvent['state'], requests: ok };
}
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
const MACHINE_OFF_FAILURES = 3;
const machineLooksOff = (id: string) => (httpFailures.get(id) ?? 0) >= MACHINE_OFF_FAILURES;
const PARKED_CHECK_FIRST_MS = 10_000;
const PARKED_CHECK_MS = 300_000;
const PARKED_CHECK_TIMEOUT_MS = 15_000;
let parkedCheckTimer: ReturnType<typeof setTimeout> | undefined;
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
let appReadySettled = false;
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

function clearServerIntegrationApprovals(id: string) {
  integrationApprovalsRaw.delete(id);
  const current = useSessions.getState().integrationApprovals;
  if (id in current) {
    const rest = { ...current };
    delete rest[id];
    useSessions.setState({ integrationApprovals: rest });
  }
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
  clearServerIntegrationApprovals(id);
  esquecerRota(id);
}

function pause() {
  generation++;
  clearTimeout(parkedCheckTimer);
  parkedCheckTimer = undefined;
  for (const id of new Set([...streams.keys(), ...retryTimers.keys(), ...connecting.keys()])) disconnect(id, false);
}

const listAnswering = (id: string) => {
  const slot = slots.get(id);
  return !!slot?.sessions && !slot.error;
};

// Todas falhando é o túnel do aparelho; só outra máquina respondendo prova que esta está fora.
function shouldPark(id: string) {
  const server = serversCache.find((s) => s.id === id);
  return !!server && !server.invite && !lastFailureHttp.get(id) && machineLooksOff(id)
    && useServers.getState().activeId !== id
    && serversCache.some((other) => other.id !== id && listAnswering(other.id));
}

function scheduleParkedCheck(delay: number, get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  if (parkedCheckTimer !== undefined || !foreground || refs === 0) return;
  if (!serversCache.some((s) => isServerParked(s.id))) return;
  parkedCheckTimer = setTimeout(() => {
    parkedCheckTimer = undefined;
    void checkParkedServers(get, set);
  }, delay);
}

async function witnessSees(witness: Server, parked: Server) {
  try {
    const id = parked.lan?.id || parked.remote?.id || parked.id;
    const { estado } = await checkPeer(witness, parked.baseUrl, id, AbortSignal.timeout(PARKED_CHECK_TIMEOUT_MS));
    return estado !== 'falhou' && estado !== 'nao_configurado';
  } catch (error) {
    registrarDiag({ evento: 'servidor.checagem_falhou', nivel: 'aviso', tela: 'lista', operacao: parked.id,
      codigo: error instanceof Error ? error.name : 'erro' });
    return false;
  }
}

// A pergunta vai às máquinas ligadas: nenhum pacote sai do aparelho para a máquina parada.
async function checkParkedServers(get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  const witnesses = serversCache.filter((s) => !s.invite && listAnswering(s.id));
  for (const s of serversCache.filter((server) => isServerParked(server.id))) {
    for (const witness of witnesses) {
      if (!await witnessSees(witness, s)) continue;
      if (isServerParked(s.id)) {
        registrarDiag({ evento: 'servidor.voltou', tela: 'lista', operacao: s.id });
        retentarAgora(s.id);
      }
      break;
    }
  }
  scheduleParkedCheck(PARKED_CHECK_MS, get, set);
}

function scheduleRetry(id: string, get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  if (!foreground || refs === 0 || retryTimers.has(id)) return;
  if (shouldPark(id)) {
    parkServer(id);
    registrarDiag({ evento: 'servidor.parado', nivel: 'aviso', tela: 'lista', operacao: id });
    scheduleParkedCheck(PARKED_CHECK_MS, get, set);
    return;
  }
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
  const tags: Record<string, TagsEvent> = {};
  for (const [id, slot] of slots) if (slot.tags) tags[id] = slot.tags;
  set({
    rows: aggCache.rows,
    byServer: aggCache.byServer,
    byServerRecord,
    tags,
    loading: aggCache.loading,
    servers: serversCache,
  });
  pairsMaybeChanged();
  reportAppReady();
}

// Uma vez por execução, com a primeira lista do servidor ativo; se ela falhou, a execução não conta.
function reportAppReady() {
  if (appReadySettled) return;
  const saved = useServers.getState();
  if (!saved.ready) return;
  // Sem servidor salvo a espera é o pareamento, não a abertura do app.
  if (!saved.activeId) { appReadySettled = true; return; }
  const server = serversCache.find((s) => s.id === saved.activeId);
  const slot = server && slots.get(server.id);
  if (!server || !slot || (!slot.error && !slot.sessions)) return;
  appReadySettled = true;
  if (!slot.error) registrarDiag({ evento: 'app.pronto', ms: Math.round(performance.now() - appStartedAt) }, server.baseUrl);
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
    if (isServerParked(s.id)) {
      slots.set(s.id, failedSlot(s.id, 'offline'));
      continue;
    }
    if (streams.has(s.id) || retryTimers.has(s.id) || connecting.has(s.id)) continue;
    if (retryDelay(s.id) > 0) {
      slots.set(s.id, failedSlot(s.id, lastFailureHttp.get(s.id) ? m.sessao_erro_servidor() : 'offline'));
      scheduleRetry(s.id, get, set);
      continue;
    }
    if (estaDesligado(s.id)) slots.set(s.id, failedSlot(s.id, 'offline'));
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
  let connectChecked = false;
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
  // Trocar o token reconecta o servidor; a rota do Connect é aprendida na conexão seguinte.
  const refreshConnect = () => {
    const current = useServers.getState().servers.find((server) => server.id === s.id);
    if (!current?.lan?.id) return;
    void upgradeToDevice(current).then(async (server) => {
      if (server.credential !== current.credential) {
        useServers.getState().rememberDevice(current.id, server, current);
        return;
      }
      const route = await discoverConnectRoute(server);
      if (route && isCurrent(attempt)) useServers.getState().rememberConnect(server.id, route, server);
    }).catch(() => {
      registrarDiag({ evento: 'connect.rota_nao_gravada', nivel: 'aviso', tela: 'lista', operacao: s.id, codigo: 'connect_route_store_failed' });
    });
  };
  const receivedList = () => {
    endOutage(s.id);
    countedHttpFailure = false;
    httpFailures.delete(s.id);
    httpRetryAt.delete(s.id);
    lastFailureHttp.delete(s.id);
    answered();
    useServers.getState().reactivateDevice(s.id);
    if (!discovered) {
      discovered = true;
      machineDiscovery(s);
    }
    refreshRemote();
    if (!connectChecked) {
      connectChecked = true;
      refreshConnect();
    }
  };
  const rejected = (status: number) => {
    if (s.invite && (status === 401 || status === 410)) {
      useServers.getState().markEnded(s.id);
      slots.delete(s.id);
      recompute(set);
      return true;
    }
    if (!s.invite && status === 401) {
      useServers.getState().markInvalid(s.id, s.token);
      // Mesmo token depois do 401 é aparelho desligado: espera Reconectar ou a volta ao app, sem insistir.
      if (useServers.getState().servers.some((server) => server.id === s.id && server.token === s.token)) {
        refused.set(s.id, status);
        slots.set(s.id, { sessions: null, error: refusedError(status) });
        recompute(set);
      }
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
    const inicio = Date.now();
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
      endOutage(s.id);
      slots.set(s.id, patchSlot(slots, s.id, { sessions: reuseUnchanged(slots.get(s.id)?.sessions, sessions), error: null, keyring: undefined }));
      recompute(set);
    } catch (error) {
      if (!isCurrent(attempt)) return;
      lastFailureHttp.set(s.id, httpResponse);
      // Só o tipo do erro: a mensagem pode trazer trecho do corpo (JSON inválido) ou o nome da máquina.
      const status = httpResponse && error instanceof Error && /^\d{3}$/.test(error.message) ? error.message : null;
      registrarDiag({ evento: 'lista.falhou', nivel: 'erro', tela: 'lista', codigo: status ?? 'sincronia',
        detalhe: `${httpResponse ? 'resposta' : 'conexao'}: ${error instanceof Error ? error.name : 'erro'}`,
        ms: Date.now() - inicio }, s.baseUrl);
      if (!httpResponse) {
        failed(true);
        // REST lento ainda se recupera pelo SSE; máquina desligada só ganharia mais uma conexão.
        if (machineLooksOff(s.id) && useServers.getState().activeId !== s.id) httpFailed();
      }
      slots.set(s.id, failedSlot(s.id, httpResponse ? m.sessao_erro_servidor() : 'offline'));
      recompute(set);
      if (countedHttpFailure) {
        esquecerRota(s.id);
        scheduleRetry(s.id, get, set);
        return;
      }
    }
    // O aparelho só alimenta a dica do celular no desktop: cofre que não responde não segura a lista ao vivo. Aberta
    // sem ele, o primeiro acesso fica para a próxima reabertura da lista. Convite não manda o aparelho: não espera.
    let identityTimer: ReturnType<typeof setTimeout> | undefined;
    const identityLate = s.invite ? false : await Promise.race([
      deviceIdentityReady().then(() => false),
      new Promise<boolean>((resolve) => { identityTimer = setTimeout(() => resolve(true), 3000); }),
    ]).finally(() => clearTimeout(identityTimer));
    if (!isCurrent(attempt)) return;
    if (identityLate) registrarDiag({ evento: 'aparelho.atrasou', nivel: 'aviso', tela: 'lista', codigo: 'device_identity_timeout' }, s.baseUrl);
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
        slots.set(s.id, patchSlot(slots, s.id, { sessions: reuseUnchanged(slots.get(s.id)?.sessions, sessions as SessionInfo[]), error: null }));
        if (falhaDados) registrarDiag({ evento: 'lista.voltou', tela: 'lista' }, s.baseUrl);
        falhaDados = false;
      } catch {
        falhaDados = true;
        registrarDiag({ evento: 'lista.falhou', nivel: 'erro', tela: 'lista', codigo: 'json_invalido' }, s.baseUrl);
        slots.set(s.id, patchSlot(slots, s.id, { error: m.sessao_erro_servidor() }));
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
      slots.set(s.id, patchSlot(slots, s.id, { sessions, error: sessions?.length ? null : m.sessao_erro_servidor() }));
      recompute(set);
    });
    // Chaveiro `login` da máquina: chega na conexão e a cada troca; frame torto mantém o último.
    es.addEventListener('keyring', (e) => {
      if (!active()) return;
      answered();
      let state: unknown;
      try { state = (JSON.parse(e.data) as { state?: unknown } | null)?.state; } catch { return; }
      if (state !== 'locked' && state !== 'unlocked' && state !== 'absent') return;
      const slot = slots.get(s.id);
      if (slot?.keyring === state) return;
      slots.set(s.id, patchSlot(slots, s.id, { keyring: state }));
      recompute(set);
    });
    // Evento guardado por máquina: chega na conexão e a cada troca. Repetido ou torto mantém o último.
    const listenKeyed = <K extends KeyedField>(name: string, raw: Map<string, string>, parse: (data: string) => KeyedEvent<K> | null, field: K) => {
      es.addEventListener(name, (e) => {
        if (!active()) return;
        answered();
        const data: string = e.data;
        if (raw.get(s.id) === data) return;
        const event = parse(data);
        if (!event) return;
        raw.set(s.id, data);
        set({ [field]: { ...get()[field], [s.id]: event } } as unknown as Pick<SessionsState, K>);
      });
    };
    // Cofre: estado e pedidos abertos.
    listenKeyed('vault', vaultRaw, parseVaultEvent, 'vault');
    // Remove aprovações da conexão anterior antes de reconectar ou ao cair o stream.
    const dropIntegrationApprovals = () => {
      integrationApprovalsRaw.delete(s.id);
      if (s.id in get().integrationApprovals) {
        const { [s.id]: _dropped, ...integrationApprovals } = get().integrationApprovals;
        set({ integrationApprovals });
      }
    };
    dropIntegrationApprovals();
    // Aprovações de integrações MCP.
    listenKeyed('integrations_approvals', integrationApprovalsRaw, parseIntegrationApprovalEvent, 'integrationApprovals');
    // Rascunhos de WhatsApp pendentes.
    listenKeyed('whatsapp_outbox', whatsappOutboxRaw, parseWhatsAppOutboxEvent, 'whatsappOutbox');
    // Etiquetas da máquina e das contas: chegam na conexão e a cada troca. Repetido ou torto mantém as últimas.
    es.addEventListener('tags', (e) => {
      if (!active()) return;
      answered();
      const data: string = e.data;
      if (slots.get(s.id)?.tagsRaw === data) return;
      const tags = parseTagsEvent(data);
      if (!tags) return;
      slots.set(s.id, patchSlot(slots, s.id, { tags, tagsRaw: data }));
      recompute(set);
    });
    // Avisos ignorados da máquina: chegam na conexão e a cada troca. Torto mantém a última lista.
    es.addEventListener('notice_ignores', (e) => {
      if (!active()) return;
      answered();
      const keys = parseNoticeIgnores(e.data);
      if (keys) setNoticeIgnores(s.id, keys);
    });
    // Revisões do trabalho da máquina: o mapa inteiro a cada troca. Repetido ou torto mantém o último.
    es.addEventListener('work_reviews', (e) => {
      if (!active()) return;
      answered();
      const data: string = e.data;
      if (workReviewsRaw.get(s.id) === data) return;
      const event = parseWorkReviewsEvent(data);
      if (!event) return;
      workReviewsRaw.set(s.id, data);
      setWorkReviewSummaries(s.id, event.reviews);
    });
    es.onerror = (event) => {
      if (!active()) return;
      dropIntegrationApprovals();
      const status = (event as { xhrStatus?: number } | null)?.xhrStatus ?? 0;
      const http = Number.isInteger(status) && status >= 100 && status <= 599;
      lastFailureHttp.set(s.id, http);
      if (http) answered();
      else failed();
      streams.delete(s.id);
      es.close();
      if (http && rejected(status)) return;
      httpFailed();
      slots.set(s.id, failedSlot(s.id, http ? m.sessao_erro_servidor() : 'offline'));
      recompute(set);
      esquecerRota(s.id);
      scheduleRetry(s.id, get, set);
    };
    streams.set(s.id, es);
  } catch (error) {
    if (!isCurrent(attempt)) return;
    integrationApprovalsRaw.delete(s.id);
    if (s.id in get().integrationApprovals) {
      const { [s.id]: _dropped, ...integrationApprovals } = get().integrationApprovals;
      set({ integrationApprovals });
    }
    lastFailureHttp.set(s.id, false);
    registrarDiag({ evento: 'lista.falhou', nivel: 'erro', tela: 'lista', codigo: 'conexao',
      detalhe: error instanceof Error ? error.name : 'erro' }, s.baseUrl);
    failed();
    httpFailed();
    slots.set(s.id, failedSlot(s.id, 'offline'));
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
    for (const s of serversCache) if (respondeuRecentemente(s.id) && !isServerParked(s.id)) {
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
      // Mesmo id com outro token ou endereço: o pedido antigo não é desta conexão.
      vaultRaw.delete(s.id);
      integrationApprovalsRaw.delete(s.id);
      whatsappOutboxRaw.delete(s.id);
      const { [s.id]: _dropped, ...vault } = get().vault;
      const { [s.id]: _droppedApproval, ...integrationApprovals } = get().integrationApprovals;
      const { [s.id]: _droppedDrafts, ...whatsappOutbox } = get().whatsappOutbox;
      set({ vault, integrationApprovals, whatsappOutbox });
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
      if (isServerParked(activeId)) {
        httpFailures.delete(activeId);
        httpRetryAt.delete(activeId);
        retentarAgora(activeId);
      }
    }
  }
  connect(serversCache, get, set);
  scheduleParkedCheck(PARKED_CHECK_FIRST_MS, get, set);
}

function start(get: () => SessionsState, set: (p: Partial<SessionsState>) => void) {
  storeGet = get;
  storeSet = set;
  unsubRecovered = onServerRecovered((id) => {
    refused.delete(id);
    failures.delete(id);
    retryAt.delete(id);
    if (machineLooksOff(id) && !lastFailureHttp.get(id)) {
      httpFailures.delete(id);
      httpRetryAt.delete(id);
    }
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
  vaultRaw.clear();
  integrationApprovalsRaw.clear();
  whatsappOutboxRaw.clear();
  workReviewsRaw.clear();
  clearWorkReviewSummaries();
  set({ vault: {}, integrationApprovals: {}, whatsappOutbox: {} });
  for (const [id, status] of refused) slots.set(id, { sessions: null, error: refusedError(status) });
  // hidden mantém? Não — limpa ao parar para próximo retain começar limpo
  hidden = new Map<string, string | null>();
  recompute(set);
}

export const useSessions = create<SessionsState>((set, get) => ({
  rows: [],
  byServer: [],
  byServerRecord: {},
  vault: {},
  integrationApprovals: {},
  whatsappOutbox: {},
  tags: {},
  noticeIgnores: {},
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
        const isActive = s.id === useServers.getState().activeId;
        // O erro falso da suspensão é uma falha só; quem já falhou seguido segue no prazo.
        if (!wasRefused && !isActive && (machineLooksOff(s.id) || isServerParked(s.id))) continue;
        httpRetryAt.delete(s.id);
        if (wasRefused || isActive || respondeuRecentemente(s.id)) {
          retentarAgora(s.id);
          retryAt.delete(s.id);
        }
      }
      connect(serversCache, get, set);
      scheduleParkedCheck(PARKED_CHECK_FIRST_MS, get, set);
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

/** Guarda a lista de avisos ignorados de um servidor; a mesma lista não troca o objeto. */
export function setNoticeIgnores(serverId: string, keys: string[]) {
  const all = useSessions.getState().noticeIgnores;
  if (all[serverId]?.join('\n') === keys.join('\n')) return;
  useSessions.setState({ noticeIgnores: { ...all, [serverId]: keys } });
}

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
  vaultRaw.clear();
  integrationApprovalsRaw.clear();
  whatsappOutboxRaw.clear();
  workReviewsRaw.clear();
  clearWorkReviewSummaries();
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
  appReadySettled = false;
  unsubServers?.();
  unsubServers = null;
  unsubRecovered?.();
  unsubRecovered = null;
  _limparEsfriamentoParaTestes();
  definirArmazem(null);
  configureCooldown();
  useSessions.setState({ rows: [], byServer: [], byServerRecord: {}, vault: {}, integrationApprovals: {}, whatsappOutbox: {}, tags: {}, noticeIgnores: {}, loading: false, servers: [] });
}
