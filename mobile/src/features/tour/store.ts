import { create } from 'zustand';
import {
  mergeTourProgress, mobileTourOutcome, planMobileTour, registrarDiag, runAction, runChoice, sameTourProgress, seenTip, seenTour,
  startTourRun, tipsSwitch, tourProgressFor, type Server, type TourChoice, type TourHomeMode, type TourProgress, type TourRun, type TourStep,
} from '@hangar/core';
import { prefs } from '../../stores/prefs';
import { ownServers, useServers } from '../../stores/servers';
import { DEMO_BASE_URL } from '../../net/demo';
import { haptic } from '../../ui/haptics';
import { listenTourActions } from './anchors';

const KEY = 'tour.progress.v1';

function loadLocal(): TourProgress {
  try {
    const raw = prefs.getString(KEY);
    return raw ? mergeTourProgress(JSON.parse(raw) as TourProgress, {}) : {};
  } catch { return {}; }
}

function saveLocal(progress: TourProgress) {
  // Sem gravar, o servidor ainda guarda: a próxima abertura junta de lá.
  try { prefs.set(KEY, JSON.stringify(progress)); } catch { /* fica só na memória */ }
}

interface TourState {
  progress: TourProgress;
  /** Ao menos um servidor respondeu ao visto: antes disso o tour já visto noutro aparelho reapareceria (offline também). */
  synced: boolean;
  /** A volta do tour já foi planejada nesta abertura; as dicas podem existir antes dela. */
  planned: boolean;
  /** "Ver o tour de novo" pedido e ainda não planejado. */
  replay: boolean;
  run: TourRun | null;
  tips: TourStep[];
}

export const useTour = create<TourState>(() => ({
  progress: loadLocal(), synced: false, planned: false, replay: false, run: null, tips: [],
}));

const servers = (): Server[] => ownServers(useServers.getState().servers).filter((s) => !s.disabled && s.baseUrl !== DEMO_BASE_URL);

const failed = (code: string, e: unknown) =>
  registrarDiag({ evento: 'tour.sync_failed', nivel: 'aviso', codigo: code, detalhe: e instanceof Error ? e.message : String(e) });

function push(patch: TourProgress) {
  for (const server of servers()) tourProgressFor(server, patch).catch((e: unknown) => failed('tour_progress_put_failed', e));
}

function record(patch: TourProgress) {
  const progress = mergeTourProgress(useTour.getState().progress, patch);
  useTour.setState({ progress });
  saveLocal(progress);
  push(patch);
}

let running: Promise<void> | null = null;
let queued: Promise<void> | null = null;

/**
 * Junta o visto de cada servidor do dono com o deste aparelho e devolve a cada um o que faltava nele.
 * Pedido com outra rodada em voo não se perde: uma rodada a mais roda depois dela (servidor novo, volta da rede).
 */
export function syncTour(): Promise<void> {
  if (!running) {
    running = syncRound()
      .catch((e: unknown) => failed('tour_progress_sync_failed', e))
      .finally(() => { running = null; });
    return running;
  }
  queued ??= running.then(() => { queued = null; return syncTour(); });
  return queued;
}

async function syncRound(): Promise<void> {
  const { ready, vaultReadable } = useServers.getState();
  const own = servers();
  const reads = await Promise.allSettled(own.map(async (server) => ({ server, remote: await tourProgressFor(server) })));
  let progress = useTour.getState().progress;
  // Cofre lido sem máquina própria (demonstração, só convite): não há visto noutro lugar para esperar.
  let answered = own.length === 0 && ready && vaultReadable;
  for (const read of reads) {
    if (read.status === 'rejected') failed('tour_progress_get_failed', read.reason);
    else {
      answered = true;
      if (read.value.remote) progress = mergeTourProgress(progress, read.value.remote);
    }
  }
  // Sem nenhuma resposta o visto continua desconhecido: o tour espera, e a próxima chamada tenta de novo.
  useTour.setState(answered ? { progress, synced: true } : { progress });
  saveLocal(progress);
  for (const read of reads) {
    if (read.status !== 'fulfilled' || !read.value.remote) continue;
    const { server, remote } = read.value;
    if (!sameTourProgress(mergeTourProgress(remote, progress), remote)) {
      tourProgressFor(server, progress).catch((e: unknown) => failed('tour_progress_put_failed', e));
    }
  }
}

/**
 * Planeja com as listas lidas e o modo da tela inicial. Sem o visto dos servidores nada é planejado,
 * nem dica; tudo espera ele chegar. O tour já visto (ou pulado) só deixa as dicas.
 */
export function planTour(sessions: number, home: TourHomeMode = 'sessions') {
  const { progress, planned, synced, replay } = useTour.getState();
  if (planned) return;
  const { steps, tips } = planMobileTour(sessions, home);
  // Sem o visto dos servidores, nem dica: quem já a viu ou desligou noutro aparelho a veria de novo.
  if (!synced && !replay) return;
  const run = replay || mobileTourOutcome(progress) === null ? startTourRun(steps) : null;
  useTour.setState({ planned: true, replay: false, run, tips });
}

function settle(next: { run: TourRun; outcome: TourProgress['tour'] }) {
  useTour.setState({ run: next.run });
  if (next.outcome) {
    record(seenTour(next.outcome));
    if (next.outcome === 'done') haptic.success();
  }
}

/** A pessoa fez a ação de um passo (segurou a sessão, tocou na busca…): só o passo na tela anda. */
export function tourAction(stepId: string) {
  const { run } = useTour.getState();
  if (!run) return;
  const before = run.index;
  const next = runAction(run, stepId);
  if (next.run.index !== before) haptic.selection();
  settle(next);
}

export function tourChoice(choice: TourChoice) {
  const { run } = useTour.getState();
  if (run) settle(runChoice(run, choice));
}

export function tipSeen(id: string) { record(seenTip(id)); }

export function setTipsOff(on: boolean) { record(tipsSwitch(on)); }

/** "Ver o tour de novo": a próxima tela com lista planeja outra volta; o visto não volta atrás. */
export function replayTour() { useTour.setState({ planned: false, replay: true, run: null }); }

listenTourActions(tourAction);
