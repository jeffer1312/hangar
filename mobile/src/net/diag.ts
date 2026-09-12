import { AppState, Platform } from 'react-native';
import { configureDiag, criarTransporteDiag, type EventoDiag } from '@hangar/core';
import { useServers } from '../stores/servers';

let iniciado = false;

export function iniciarDiag(): void {
  if (iniciado) return;
  iniciado = true;
  const cli = Math.random().toString(36).slice(2, 8);
  let seq = 0;
  const destinos = new Set<string>();
  const transporte = criarTransporteDiag((destino) => useServers.getState().servers
    .find((s) => s.baseUrl.replace(/\/+$/, '') === destino)?.token);
  const registrar = (evento: EventoDiag, destino = useServers.getState().active()?.baseUrl) => {
    if (!destino) return;
    const base = destino.replace(/\/+$/, '');
    if (!destinos.has(base)) {
      destinos.add(base);
      transporte.registrar(base, { evento: 'app.abriu', cli, seq: ++seq,
        ts: new Date().toISOString(), so: Platform.OS, navegador: 'Expo', vista: 'celular' });
    }
    transporte.registrar(base, { ...evento, cli, seq: ++seq, ts: new Date().toISOString() });
  };
  configureDiag({ registrar, novoReq: () => `${cli}-${(++seq).toString(36)}` });
  useServers.subscribe(() => { void transporte.enviar(); });
  AppState.addEventListener('change', (estado) => {
    registrar({ evento: estado === 'active' ? 'app.visivel' : 'app.oculto' });
    void transporte.enviar();
  });
}
