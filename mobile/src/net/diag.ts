import { AppState, Platform } from 'react-native';
import { updateId } from 'expo-updates';
import { baseOf, configureDiag, criarTransporteDiag, isOwnMachine, isServerParked, registrarDiag, type EventoDiag } from '@hangar/core';
import { useServers } from '../stores/servers';
import { replayFatalError } from './fatalErrors';

let iniciado = false;

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
        detalhe: updateId ?? 'embutido' });
    }
    transporte.registrar(base, { ...evento, cli, seq: ++seq, ts: new Date().toISOString() });
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
      const response = await fetch(`${baseOf(server).replace(/\/+$/, '')}/api/diag`, {
        method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${server.token}` },
        body: JSON.stringify({ eventos: [event] }), signal: AbortSignal.timeout(8000),
      });
      if (!response.ok) return false;
      const ack: unknown = await response.json();
      return !!ack && typeof ack === 'object' && 'gravadas' in ack && ack.gravadas === 1;
    }).catch(() => { console.warn('fatal_error_diary_replay_failed'); });
  };
  replay();
  useServers.subscribe(() => { void transporte.enviar(); replay(); });
  AppState.addEventListener('change', (estado) => {
    registrar({ evento: estado === 'active' ? 'app.visivel' : 'app.oculto' });
    void transporte.enviar();
    if (estado === 'active') replay();
  });
}
