import { useCallback, useEffect, useState } from 'react';
import { AppState } from 'react-native';
import { useFocusEffect } from 'expo-router';
import { STATUS_EVENT, getStatus, isServerParked, openStatusStream, parseStatusEvent, registrarDiag,
  type EventSourceLike, type StatusSnapshot } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';

export type StatusFailure = { code?: number; detail: string };

export type StatusView =
  | { kind: 'none' }
  | { kind: 'loading'; label: string }
  | { kind: 'down'; label: string; since?: number }
  | { kind: 'error'; label: string; failure: StatusFailure }
  | { kind: 'ok'; label: string; snap: StatusSnapshot };

const RETRY_MS = [1000, 2000, 5000, 10000];
const noop = () => {};

function failureOf(e: unknown): StatusFailure {
  const code = (e as { status?: unknown })?.status;
  return { code: typeof code === 'number' ? code : undefined, detail: e instanceof Error ? e.message : String(e) };
}

/**
 * Retrato do servidor ativo e o SSE dele, só com a tela em foco, o app em primeiro plano e o servidor
 * respondendo. Estacionado ou fora do ar, nada sai do aparelho: a tela só informa.
 */
export function useStatus(): { view: StatusView; retry: () => void } {
  const server = useServers((s) => s.servers.find((x) => x.id === s.activeId) ?? null);
  const bucket = useSessions((s) => s.byServer.find((b) => b.server.id === server?.id));
  const offline = bucket?.error === 'offline';
  const down = !!server && (offline || isServerParked(server.id));
  const [foreground, setForeground] = useState(AppState.currentState === 'active');
  const [attempt, setAttempt] = useState(0);
  const [data, setData] = useState<{ id: string; snap?: StatusSnapshot; failure?: StatusFailure } | null>(null);

  // A lista ao vivo é quem diz que o servidor caiu ou voltou.
  useEffect(() => useSessions.getState().retain(), []);
  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => setForeground(state === 'active'));
    return () => sub.remove();
  }, []);

  useFocusEffect(useCallback(() => {
    if (!server || down || !foreground) return;
    let alive = true;
    let source: EventSourceLike | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let tries = 0;
    let had = false;
    const later = () => {
      if (!alive || timer) return;
      timer = setTimeout(() => { timer = null; if (alive) void load(); }, RETRY_MS[Math.min(tries++, RETRY_MS.length - 1)]);
    };
    const open = () => {
      source = openStatusStream(server);
      source.addEventListener('ping', noop);
      source.addEventListener(STATUS_EVENT, (ev) => {
        const snap = parseStatusEvent(ev.data);
        if (!snap || !alive) return;
        tries = 0;
        setData({ id: server.id, snap });
      });
      // Caiu (rede, servidor reiniciando): relê o retrato e reabre com espera crescente.
      source.onerror = () => { source?.close(); source = null; later(); };
    };
    const load = async () => {
      try {
        const snap = await getStatus(server);
        if (!alive) return;
        had = true;
        setData({ id: server.id, snap });
        open();
      } catch (e) {
        if (!alive) return;
        const failure = failureOf(e);
        registrarDiag({ evento: 'status.leitura_falhou', nivel: 'aviso', codigo: failure.code ? String(failure.code) : 'rede' });
        // Releitura depois de uma queda do SSE: o último retrato fica e tenta de novo; só a primeira vira erro.
        if (had) later();
        else setData({ id: server.id, failure });
      }
    };
    void load();
    return () => { alive = false; if (timer) clearTimeout(timer); source?.close(); };
  }, [server, down, foreground, attempt]));

  const retry = useCallback(() => {
    setData(null);
    if (down) useSessions.getState().reconnect();
    setAttempt((n) => n + 1);
  }, [down]);

  if (!server) return { view: { kind: 'none' }, retry };
  const label = server.label;
  if (down) return { view: { kind: 'down', label, since: bucket?.offlineSince }, retry };
  const mine = data?.id === server.id ? data : null;
  if (mine?.snap) return { view: { kind: 'ok', label, snap: mine.snap }, retry };
  if (mine?.failure) return { view: { kind: 'error', label, failure: mine.failure }, retry };
  return { view: { kind: 'loading', label }, retry };
}
