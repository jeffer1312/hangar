import { create } from 'zustand';
import {
  mergeTourProgress, mobileTourOutcome, planMobileTour, registrarDiag, runAction, runChoice, sameTourProgress, seenTip, seenTour,
  startTourRun, tipsSwitch, tourProgressFor, type Server, type TourChoice, type TourProgress, type TourRun, type TourStep,
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
  /** A primeira leitura dos servidores terminou (ou falhou): antes disso o tour visto noutro aparelho reapareceria. */
  synced: boolean;
  planned: boolean;
  run: TourRun | null;
  tips: TourStep[];
}

export const useTour = create<TourState>(() => ({
  progress: loadLocal(), synced: false, planned: false, run: null, tips: [],
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

let syncing: Promise<void> | null = null;

/** Junta o visto de cada servidor do dono com o deste aparelho e devolve a cada um o que faltava nele. */
export function syncTour(): Promise<void> {
  if (syncing) return syncing;
  syncing = (async () => {
    const reads = await Promise.allSettled(servers().map(async (server) => ({ server, remote: await tourProgressFor(server) })));
    let progress = useTour.getState().progress;
    for (const read of reads) {
      if (read.status === 'rejected') failed('tour_progress_get_failed', read.reason);
      else if (read.value.remote) progress = mergeTourProgress(progress, read.value.remote);
    }
    useTour.setState({ progress, synced: true });
    saveLocal(progress);
    for (const read of reads) {
      if (read.status !== 'fulfilled' || !read.value.remote) continue;
      const { server, remote } = read.value;
      if (!sameTourProgress(mergeTourProgress(remote, progress), remote)) {
        tourProgressFor(server, progress).catch((e: unknown) => failed('tour_progress_put_failed', e));
      }
    }
  })().finally(() => { syncing = null; });
  return syncing;
}

/** Planeja a volta com a lista lida. O tour já visto (ou pulado) só deixa as dicas. */
export function planTour(sessions: number, replay = false) {
  const { progress, planned } = useTour.getState();
  if (planned && !replay) return;
  const { steps, tips } = planMobileTour(sessions);
  const run = replay || mobileTourOutcome(progress) === null ? startTourRun(steps) : null;
  useTour.setState({ planned: true, run, tips });
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
export function replayTour() { useTour.setState({ planned: false, run: null }); replayRequested = true; }
let replayRequested = false;
export function takeReplay(): boolean { const asked = replayRequested; replayRequested = false; return asked; }

listenTourActions(tourAction);
