import { AppState, Platform } from 'react-native';
import { updateId } from 'expo-updates';
import Constants from 'expo-constants';
import { baseOf, configureDiag, criarTransporteDiag, isOwnMachine, isServerParked, registrarDiag, type EventoDiag } from '@hangar/core';
import { useServers } from '../stores/servers';
import { fatalTelemetryEvent, replayFatalError } from './fatalErrors';

let iniciado = false;
// O tempo na tela sai a cada 5 min; o servidor recusa mais que isso num evento.
const VISIBLE_EVERY_MS = 5 * 60 * 1000;

export function withMachineRequestDiagnostics(fetcher: typeof fetch): typeof fetch {
  return async (input, init) => {
    let url: URL;
    try { url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url); }
    catch { return fetcher(input, init); }
    if (!/^\/api\/(peers(?:\/.*)?|sessions)$/.test(url.pathname)) return fetcher(input, init);
    const started = Date.now();
    const own = useServers.getState().servers.filter(isOwnMachine);
    const server = own.find((s) => url.href.startsWith(`${baseOf(s)}/api/`)) ?? own[0];
    const report = (code: string) => server && registrarDiag({
      evento: 'machines.request_failed', nivel: 'aviso', codigo: code, ms: Date.now() - started,
      operacao: url.host,
      detalhe: `${init?.method ?? 'GET'} ${url.pathname}`,
    }, baseOf(server));
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

export function iniciarDiag(): void {
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
    const server = useServers.getState().servers
      .find((s) => s.baseUrl.replace(/\/+$/, '') === destino || baseOf(s) === destino);
    return server && !isServerParked(server.id) ? server.token : null;
  });
  // O diário fica nas máquinas desta pessoa, nunca na entrada do par de outra pessoa.
  const ativaPropria = () => { const a = useServers.getState().active(); return a && !a.invite ? a.baseUrl : undefined; };
  const registrar = (evento: EventoDiag, destino = ativaPropria()) => {
    if (!destino) return;
    const base = destino.replace(/\/+$/, '');
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
      const server = useServers.getState().active();
      if (!server || server.invite || !server.token) return false;
      const event = {
        evento: 'js.fatal', nivel: 'erro', ts: report.timestamp, cli, seq: ++seq,
        codigo: report.name, tela: report.route, detalhe: report.message.slice(0, 300),
        pilha: (report.stack || report.componentStack).slice(0, 300),
        operacao: [report.metadata.runtime, report.metadata.updateId].filter(Boolean).join(' · '),
      };
      const eventos = [event, { ...fatalTelemetryEvent(report), cliente: client, cli, seq: ++seq }];
      const response = await fetch(`${baseOf(server).replace(/\/+$/, '')}/api/diag`, {
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
    if (ms > 0) registrar({ evento: 'app.tempo', ms });
  };
  setInterval(() => { visibleTime(); void transporte.enviar(); }, VISIBLE_EVERY_MS);
  useServers.subscribe(() => { void transporte.enviar(); replay(); });
  AppState.addEventListener('change', (estado) => {
    if (estado === 'active') { if (visibleSince === null) visibleSince = Date.now(); }
    else { visibleTime(); visibleSince = null; }
    registrar({ evento: estado === 'active' ? 'app.visivel' : 'app.oculto' });
    void transporte.enviar();
    if (estado === 'active') replay();
  });
}
