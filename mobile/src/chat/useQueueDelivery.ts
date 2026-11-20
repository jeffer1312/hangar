import { useEffect } from 'react';
import { create } from 'zustand';
import type { Server } from '@hangar/core';
import { prefs } from '../stores/prefs';
import { toast } from '../ui/Toast';
import * as m from '../paraglide/messages';
import { readQueueJev, type DeliveryControls, type DeliveryMode, type JevStatus } from './queueDelivery';

const KEY = 'queue.deliveryMode';
const saved = prefs.getString(KEY);
export const useDeliveryMode = create<{ mode: DeliveryMode; setMode: (mode: DeliveryMode) => void }>((set) => ({
  mode: saved === 'direct' || saved === 'jev' || saved === 'conversation' ? saved : 'queued',
  // Sem gravar no aparelho a escolha ainda vale até fechar o app, e quem escolheu precisa saber disso.
  setMode: (mode) => {
    try { prefs.set(KEY, mode); } catch { toast.erro(m.mobile_queue_mode_save_failed()); }
    set({ mode });
  },
}));

const POLL_MS = 30_000;
const RETRY_MS = 5_000;
type Availability = { status: JevStatus; error?: string; failures: number };
// Uma leitura por servidor, dividida entre composer e pergunta: a disponibilidade do Jev é do servidor.
const useAvailability = create<Record<string, Availability>>(() => ({}));
type Loop = { server: Server; refs: number; controller?: AbortController; timer?: ReturnType<typeof setTimeout> };
const loops = new Map<string, Loop>();
let foreground = true;

function stop(loop: Loop) {
  loop.controller?.abort();
  clearTimeout(loop.timer);
}

async function read(identity: string, loop: Loop) {
  stop(loop);
  if (!foreground || loops.get(identity) !== loop) return;
  const controller = new AbortController();
  loop.controller = controller;
  let next = POLL_MS;
  try {
    const available = await readQueueJev(loop.server, controller.signal);
    if (controller.signal.aborted) return;
    useAvailability.setState({ [identity]: { status: available ? 'ready' : 'setup', failures: 0 } });
  } catch (error) {
    if (controller.signal.aborted) return;
    const previous = useAvailability.getState()[identity];
    const failures = (previous?.failures ?? 0) + 1;
    const message = error instanceof Error ? error.message : String(error);
    const unsupported = (error as { status?: number })?.status === 404;
    // Uma falha passageira não tira o Jev de quem já o tinha pronto; confere de novo logo e só cai na segunda seguida.
    if (previous?.status === 'ready' && failures < 2 && !unsupported) {
      useAvailability.setState({ [identity]: { ...previous, failures } });
      next = RETRY_MS;
    } else {
      useAvailability.setState({ [identity]: { status: unsupported ? 'unsupported' : 'error', error: message, failures } });
    }
  }
  loop.timer = setTimeout(() => void read(identity, loop), next);
}

function acquire(identity: string, server: Server) {
  const existing = loops.get(identity);
  if (existing) { existing.refs++; return; }
  const loop: Loop = { server, refs: 1 };
  loops.set(identity, loop);
  void read(identity, loop);
}

function release(identity: string) {
  const loop = loops.get(identity);
  if (!loop || --loop.refs > 0) return;
  stop(loop);
  loops.delete(identity);
  useAvailability.setState((all) => {
    const { [identity]: _gone, ...rest } = all;
    return rest;
  }, true);
}

// Em segundo plano ninguém vê o modo: para de conferir e confere na hora da volta.
export function setQueueDeliveryForeground(active: boolean): void {
  if (active === foreground) return;
  foreground = active;
  for (const [identity, loop] of loops) {
    if (!active) { stop(loop); continue; }
    // A falha de antes da pausa não conta contra a volta.
    const previous = useAvailability.getState()[identity];
    if (previous?.failures) useAvailability.setState({ [identity]: { ...previous, failures: 0 } });
    void read(identity, loop);
  }
}

export function useQueueDelivery(server: Server | undefined, enabled: boolean): DeliveryControls {
  const mode = useDeliveryMode((s) => s.mode);
  const identity = JSON.stringify([server?.id, server?.baseUrl, server?.token]);
  const current = useAvailability((all) => (server && enabled ? all[identity] : undefined));
  useEffect(() => {
    if (!server || !enabled) return;
    acquire(identity, server);
    return () => release(identity);
  }, [identity, enabled]);
  // Mesma identidade com outros campos de rota: a próxima leitura usa o servidor de agora.
  useEffect(() => {
    const loop = loops.get(identity);
    if (loop && server) loop.server = server;
  });
  const status = current?.status ?? 'loading';
  const help = status === 'ready' ? m.native_queue_jev_help() : status === 'setup' ? m.native_queue_jev_setup()
    : status === 'unsupported' ? m.native_queue_jev_unsupported() : status === 'error'
      ? m.native_queue_jev_read_error({ error: current?.error ?? '' }) : m.native_queue_jev_loading();
  return { mode, status, help };
}
