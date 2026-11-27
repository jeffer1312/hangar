import { AppState, Platform } from 'react-native';
import { updateId } from 'expo-updates';
import Constants from 'expo-constants';
import { baseOf, configureDiag, criarTransporteDiag, getTelemetry, isOwnMachine, isServerParked, registrarDiag,
  SETTINGS_CONSENT_SCOPE, subscribeRoutes, UNCONFIRMED_BASE, type EventoDiag, type Server, type TelemetrySetting } from '@hangar/core';
import { useServers } from '../stores/servers';
import { fatalTelemetryEvent, replayFatalError } from './fatalErrors';

let iniciado = false;
// O tempo na tela sai neste compasso; o servidor recusa mais que isso num evento.
const VISIBLE_EVERY_MS = 5 * 60 * 1000;
const trimBase = (base: string) => base.replace(/\/+$/, '');
const destination = (server: Server) => trimBase(server.baseUrl);
type SettingsSnapshot = Pick<EventoDiag, 'values' | 'device'> & {
  cliente: 'ios' | 'android'; ts: string; snapshot_period: string;
};
let telemetrySaved: ((server: Server, state: TelemetrySetting) => void) | undefined;

export function onTelemetrySaved(server: Server, state: TelemetrySetting): void {
  telemetrySaved?.(server, state);
}

export function withMachineRequestDiagnostics(fetcher: typeof fetch): typeof fetch {
  return async (input, init) => {
    let url: URL;
    try { url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url); }
    catch { return fetcher(input, init); }
    if (!/^\/api\/(peers(?:\/.*)?|sessions)$/.test(url.pathname)) return fetcher(input, init);
    const started = Date.now();
    const own = useServers.getState().servers.filter(isOwnMachine);
    const server = own.find((s) => url.href.startsWith(`${trimBase(baseOf(s))}/api/`)
      || url.href.startsWith(`${destination(s)}/api/`)) ?? own[0];
    const report = (code: string) => server && registrarDiag({
      evento: 'machines.request_failed', nivel: 'aviso', codigo: code, ms: Date.now() - started,
      operacao: url.host,
      detalhe: `${init?.method ?? 'GET'} ${url.pathname}`,
    }, destination(server));
    try {
      const response = await fetcher(input, init);
      if (!response.ok) report(String(response.status));
      return response;
    } catch (error) {
      const name = error instanceof Error ? error.name : '';
      const signal = init?.signal;
      report(name === 'TimeoutError' || signal?.aborted && signal.reason?.name === 'TimeoutError'
        ? 'timeout' : name === 'AbortError' || signal?.aborted ? 'cancelled' : 'network');
      throw error;
    }
  };
}

/** `settings`: o retrato das preferências do aparelho, mandado uma vez por dia (UTC) ao servidor ativo. */
export function iniciarDiag(settings?: () => Pick<EventoDiag, 'values' | 'device'>): void {
  if (iniciado) return;
  iniciado = true;
  // O core resume a falha no resultado; registra aqui antes de perder o motivo do cancelamento.
  globalThis.fetch = withMachineRequestDiagnostics(globalThis.fetch.bind(globalThis));
  const cli = Math.random().toString(36).slice(2, 8);
  // A telemetria do servidor separa o tempo de tela e as rotas por cliente; o diário descarta o campo.
  const client = Platform.OS === 'ios' ? 'ios' : 'android';
  let seq = 0;
  const destinos = new Set<string>();
  const transporte = criarTransporteDiag((destino) => {
    const server = useServers.getState().servers.find((s) => destination(s) === destino && isOwnMachine(s));
    if (!server || isServerParked(server.id)) return null;
    const baseUrl = trimBase(baseOf(server));
    return baseUrl === UNCONFIRMED_BASE ? null : { baseUrl, token: server.token };
  });
  // A chave fica na máquina original, mesmo quando a rede local passa a responder.
  const ownActive = () => { const server = useServers.getState().active(); return server && isOwnMachine(server) ? server : undefined; };
  const registrar = (evento: EventoDiag, target?: string) => {
    const active = ownActive();
    let base: string;
    if (target === undefined) {
      if (!active) return;
      base = destination(active);
    } else {
      const route = trimBase(target);
      // Eventos do SSE trazem a rota usada; traduza para a identidade antes de enfileirar.
      const own = useServers.getState().servers.filter(isOwnMachine);
      const server = own.find((s) => destination(s) === route)
        ?? own.find((s) => trimBase(baseOf(s)) === route && route !== UNCONFIRMED_BASE);
      if (!server) return;
      base = destination(server);
    }
    if (!destinos.has(base)) {
      destinos.add(base);
      transporte.registrar(base, { evento: 'app.abriu', cli, seq: ++seq,
        ts: new Date().toISOString(), so: Platform.OS, navegador: 'Expo', vista: 'celular',
        versao: Constants.expoConfig?.version, detalhe: updateId ?? 'embutido' });
    }
    transporte.registrar(base, { ...evento, cliente: client, cli, seq: ++seq, ts: new Date().toISOString() });
  };
  configureDiag({ registrar, novoReq: () => `${cli}-${(++seq).toString(36)}` });
  const replay = () => {
    void replayFatalError(async (report) => {
      const server = ownActive();
      if (!server || !server.token) return false;
      const base = trimBase(baseOf(server));
      if (base === UNCONFIRMED_BASE) return false;
      const event = {
        evento: 'js.fatal', nivel: 'erro', ts: report.timestamp, cli, seq: ++seq,
        codigo: report.name, tela: report.route, detalhe: report.message.slice(0, 300),
        pilha: (report.stack || report.componentStack).slice(0, 300),
        operacao: [report.metadata.runtime, report.metadata.updateId].filter(Boolean).join(' · '),
      };
      const eventos = [event, { ...fatalTelemetryEvent(report), cliente: client, cli, seq: ++seq }];
      const response = await fetch(`${base}/api/diag`, {
        method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${server.token}` },
        body: JSON.stringify({ eventos }), signal: AbortSignal.timeout(8000),
      });
      if (!response.ok) return false;
      const ack: unknown = await response.json();
      return !!ack && typeof ack === 'object' && 'gravadas' in ack && ack.gravadas === eventos.length;
    }).catch(() => { console.warn('fatal_error_diary_replay_failed'); });
  };
  replay();
  // Só ao servidor ativo (o `registrar` sem destino); fora da tela não conta.
  let visibleSince: number | null = (AppState.currentState ?? 'active') === 'active' ? Date.now() : null;
  // Só minutos inteiros saem: o servidor arredonda cada evento, e idas curtas ao fundo somariam zero.
  let visibleRest = 0;
  const visibleTime = () => {
    if (visibleSince === null) return;
    const now = Date.now();
    visibleRest = Math.min(visibleRest + now - visibleSince, VISIBLE_EVERY_MS);
    visibleSince = now;
    const ms = visibleRest - (visibleRest % 60_000);
    visibleRest -= ms;
    if (ms > 0) registrar({ evento: 'app.visible_time', ms });
  };
  // A confirmação deste POST não se confunde com a baixa das entradas do diário.
  const settingsDay = new Map<string, string>();
  const inFlight = new Map<string, { server: Server }>();
  const snapshotTarget = () => {
    const server = ownActive();
    return server?.token && !isServerParked(server.id) && baseOf(server) !== UNCONFIRMED_BASE ? server : undefined;
  };
  const sameServer = (a: Server, b: Server | undefined) => !!b && a.id === b.id && a.token === b.token && destination(a) === destination(b);
  const settingsSnapshot = async (saved?: TelemetrySetting) => {
    const server = snapshotTarget();
    if (!settings || !server) return;
    const key = destination(server);
    // Dia já confirmado: só o dia novo ou um aceite salvo aqui (`saved`) pedem outra leitura.
    if (!saved && settingsDay.get(key)?.startsWith(`${server.id}:${new Date().toISOString().slice(0, 10)}:`)) return;
    const existing = inFlight.get(key);
    if (existing && sameServer(server, existing.server)) return;
    const flight = { server };
    inFlight.set(key, flight);
    const current = () => inFlight.get(key) === flight && sameServer(server, snapshotTarget());
    try {
      const state = saved ?? await getTelemetry(server);
      const period = state?.snapshot_period;
      if (!current() || !period || state?.consent_scope !== SETTINGS_CONSENT_SCOPE || state.disabled_by) return;
      const ts = new Date().toISOString();
      const mark = `${server.id}:${ts.slice(0, 10)}:${period}`;
      if (settingsDay.get(key) === mark) return;
      const body: SettingsSnapshot = { ...settings(), cliente: client, ts, snapshot_period: period };
      const active = snapshotTarget();
      if (!active || !current()) return;
      const response = await fetch(`${trimBase(baseOf(active))}/api/telemetry`, {
        method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${active.token}` },
        body: JSON.stringify(body), signal: AbortSignal.timeout(8000),
      });
      if (!response.ok) {
        if (current() && response.status >= 500) console.warn('telemetry_settings_snapshot_failed');
        return;
      }
      const ack: unknown = await response.json();
      if (current() && ack && typeof ack === 'object' && 'snapshot_period' in ack && ack.snapshot_period === period) {
        settingsDay.set(key, mark);
      }
    } catch {
      // O próximo ciclo confere o aceite de novo, sem conservar os valores de uma captura recusada.
      if (current()) console.warn('telemetry_settings_snapshot_failed');
    } finally {
      if (inFlight.get(key) === flight) inFlight.delete(key);
    }
  };
  telemetrySaved = (server, state) => {
    if (!sameServer(server, ownActive())) return;
    inFlight.delete(destination(server));
    if (state.snapshot_period) void settingsSnapshot(state);
  };
  let lastRoute = '';
  const checkActiveRoute = () => {
    const s = snapshotTarget();
    const route = s ? JSON.stringify([s.id, destination(s), baseOf(s), s.token]) : '';
    if (route !== lastRoute) { lastRoute = route; settingsSnapshot(); }
  };
  checkActiveRoute();
  setInterval(() => { visibleTime(); settingsSnapshot(); void transporte.enviar(); }, VISIBLE_EVERY_MS);
  useServers.subscribe(() => { checkActiveRoute(); void transporte.enviar(); replay(); });
  subscribeRoutes(() => { checkActiveRoute(); void transporte.enviar(); });
  AppState.addEventListener('change', (estado) => {
    if (estado === 'active') { if (visibleSince === null) visibleSince = Date.now(); }
    else { visibleTime(); visibleSince = null; }
    registrar({ evento: estado === 'active' ? 'app.visivel' : 'app.oculto' });
    if (estado === 'active') settingsSnapshot();
    void transporte.enviar();
    if (estado === 'active') replay();
  });
}
