import { useCallback, useEffect, useRef, useState } from 'react';
import { AppState } from 'react-native';
import { useFocusEffect } from 'expo-router';
import { STATUS_EVENT, baseOf, getStatus, isServerParked, openStatusStream, parseStatusEvent, registrarDiag,
  type EventSourceLike, type StatusSnapshot } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';

/** `reason` é o `code` do erro (ex.: `server_identity_unconfirmed`), quando ele vem. */
export type StatusFailure = { code?: number; reason?: string; detail: string };

export type StatusView =
  | { kind: 'none' }
  | { kind: 'loading'; label: string }
  | { kind: 'down'; label: string; since?: number }
  | { kind: 'error'; label: string; failure: StatusFailure }
  | { kind: 'ok'; label: string; snap: StatusSnapshot };

const RETRY_MS = [1000, 2000, 5000, 10000];
// O servidor manda `ping` a cada 10 s: dois perdidos e mais folga é conexão meio morta.
export const STATUS_WATCHDOG_MS = 25_000;

function failureOf(e: unknown): StatusFailure {
  const code = (e as { status?: unknown })?.status;
  const reason = (e as { code?: unknown })?.code;
  return {
    code: typeof code === 'number' ? code : undefined,
    reason: typeof reason === 'string' ? reason : undefined,
    detail: e instanceof Error ? e.message : String(e),
  };
}

/** Retrato novo com cada bloco igual ao anterior na mesma referência: os cartões em `memo` não redesenham. */
export function keepUnchanged(prev: StatusSnapshot | undefined, next: StatusSnapshot): StatusSnapshot {
  if (!prev) return next;
  const out: Record<string, unknown> = { ...next };
  for (const key of Object.keys(next) as (keyof StatusSnapshot)[]) {
    if (JSON.stringify(prev[key]) === JSON.stringify(next[key])) out[key] = prev[key];
  }
  return out as unknown as StatusSnapshot;
}

/**
 * Retrato do servidor ativo e o SSE dele, só com a tela em foco, o app em primeiro plano e o servidor
 * respondendo. Estacionado ou fora do ar, nada sai do aparelho: a tela só informa.
 */
export function useStatus(): { view: StatusView; retry: () => void } {
  const server = useServers((s) => s.servers.find((x) => x.id === s.activeId) ?? null);
  // O store recria o servidor quando muda outro campo (rótulo); o SSE só reabre quando muda o que a
  // conexão usa: id, token e endereço.
  const serverRef = useRef(server);
  serverRef.current = server;
  const serverId = server?.id;
  const token = server?.token;
  const base = server ? baseOf(server) : null;
  const bucket = useSessions((s) => s.byServer.find((b) => b.server.id === server?.id));
  const offline = bucket?.error === 'offline';
  // `parked` só força o redesenho: o estacionamento não avisa, quem o descobre é o laço abaixo.
  const [, setParked] = useState(0);
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
    const server = serverRef.current;
    if (!server || down || !foreground) return;
    const current = () => serverRef.current ?? server;
    let alive = true;
    let source: EventSourceLike | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let watchdog: ReturnType<typeof setTimeout> | null = null;
    let tries = 0;
    let had = false;
    const unwatch = () => { if (watchdog) clearTimeout(watchdog); watchdog = null; };
    const close = () => { unwatch(); source?.close(); source = null; };
    // Estacionado com a tela aberta: para tudo e redesenha, e o render passa a mostrar "parado".
    const stopIfParked = () => {
      if (!isServerParked(server.id)) return false;
      close();
      if (timer) clearTimeout(timer);
      timer = null;
      alive = false;
      setParked((n) => n + 1);
      return true;
    };
    const later = () => {
      if (!alive || timer || stopIfParked()) return;
      timer = setTimeout(() => { timer = null; if (alive && !stopIfParked()) void load(); }, RETRY_MS[Math.min(tries++, RETRY_MS.length - 1)]);
    };
    // Caiu ou ficou mudo (rede, servidor reiniciando): relê o retrato e reabre com espera crescente.
    const drop = () => { close(); later(); };
    const arm = () => {
      unwatch();
      watchdog = setTimeout(() => { watchdog = null; drop(); }, STATUS_WATCHDOG_MS);
    };
    const open = () => {
      if (stopIfParked()) return;
      source = openStatusStream(current());
      arm();
      source.addEventListener('ping', () => { if (alive && !stopIfParked()) arm(); });
      source.addEventListener(STATUS_EVENT, (ev) => {
        if (!alive || stopIfParked()) return;
        arm();
        const snap = parseStatusEvent(ev.data);
        if (!snap) return;
        tries = 0;
        setData((d) => ({ id: server.id, snap: keepUnchanged(d?.id === server.id ? d.snap : undefined, snap) }));
      });
      source.onerror = drop;
    };
    const load = async () => {
      try {
        const snap = await getStatus(current());
        if (!alive) return;
        had = true;
        setData((d) => ({ id: server.id, snap: keepUnchanged(d?.id === server.id ? d.snap : undefined, snap) }));
        open();
      } catch (e) {
        if (!alive) return;
        const failure = failureOf(e);
        registrarDiag({ evento: 'status.leitura_falhou', nivel: 'aviso', codigo: failure.reason ?? (failure.code ? String(failure.code) : 'rede') });
        // Releitura depois de uma queda do SSE: o último retrato fica e tenta de novo; só a primeira vira erro.
        if (had) later();
        else setData({ id: server.id, failure });
      }
    };
    void load();
    return () => { alive = false; if (timer) clearTimeout(timer); close(); };
  }, [serverId, token, base, down, foreground, attempt]));

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
