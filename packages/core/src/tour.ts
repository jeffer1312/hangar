import { apiFetchForServer } from './api';
import { isMissingRoute } from './pluginUi';
import type { Server } from './servers';

/**
 * O que a pessoa já viu do tour e das dicas: o formato do `/api/tour-progress`, o mesmo do nativo
 * (`desktop-native/src/tour/state.rs`). A junção nunca desfaz o que algum aparelho já viu.
 */
export type TourOutcome = 'skipped' | 'done';
export interface TourSwitch { on: boolean; at: string }
export interface TourProgress { tour?: TourOutcome | null; tips?: string[]; tips_off?: TourSwitch | null }

/** `tour` do contrato é o do tour do PC: o do celular vai em `tips`, que é união e nunca desfaz. */
export const MOBILE_TOUR_DONE = 'mobile-tour:done';
export const MOBILE_TOUR_SKIPPED = 'mobile-tour:skipped';
const MOBILE_TIP_PREFIX = 'mobile-tip:';

const instant = (s: TourSwitch | null | undefined) => {
  const t = s ? Date.parse(s.at) : NaN;
  return Number.isNaN(t) ? null : t;
};

/** A ordem é a da junção: `done` vence `skipped`, e nada perde para os dois. */
const rank = (o: TourOutcome | null | undefined) => (o === 'done' ? 2 : o === 'skipped' ? 1 : 0);

export function mergeTourProgress(a: TourProgress, b: TourProgress): TourProgress {
  const tour = rank(b.tour) > rank(a.tour) ? b.tour : a.tour;
  const tips = [...new Set([...(a.tips ?? []), ...(b.tips ?? [])])].sort();
  let tipsOff = a.tips_off ?? null;
  if (b.tips_off) {
    const ours = instant(tipsOff);
    const theirs = instant(b.tips_off);
    if (!tipsOff || (theirs !== null && (ours === null || theirs > ours))) tipsOff = b.tips_off;
  }
  return { tour: tour ?? null, tips, tips_off: tipsOff };
}

export function sameTourProgress(a: TourProgress, b: TourProgress): boolean {
  const x = mergeTourProgress(a, {});
  const y = mergeTourProgress(b, {});
  return x.tour === y.tour && x.tips!.join('\n') === y.tips!.join('\n')
    && (x.tips_off?.on ?? null) === (y.tips_off?.on ?? null) && (x.tips_off?.at ?? null) === (y.tips_off?.at ?? null);
}

export function mobileTourOutcome(p: TourProgress): TourOutcome | null {
  const tips = p.tips ?? [];
  if (tips.includes(MOBILE_TOUR_DONE)) return 'done';
  if (tips.includes(MOBILE_TOUR_SKIPPED)) return 'skipped';
  return null;
}

export const tourTipsOff = (p: TourProgress) => p.tips_off?.on === true;
export const mobileTipKey = (id: string) => MOBILE_TIP_PREFIX + id;
export const tourTipSeen = (p: TourProgress, id: string) => (p.tips ?? []).includes(mobileTipKey(id));

/** O que acrescentar ao visto: o PUT manda só isto, e o servidor junta. */
export const seenTip = (id: string): TourProgress => ({ tips: [mobileTipKey(id)] });
export const seenTour = (outcome: TourOutcome): TourProgress => ({ tips: [outcome === 'done' ? MOBILE_TOUR_DONE : MOBILE_TOUR_SKIPPED] });
export const tipsSwitch = (on: boolean, now: Date = new Date()): TourProgress => ({ tips_off: { on, at: now.toISOString() } });

export type TourNeed = 'emptyList' | 'sessions' | 'none';
/** A tela onde a peça mora: a inicial (lista) ou a conversa. */
export type TourScreen = 'home' | 'chat';
export interface TourStep { id: string; anchor: string; need: TourNeed; screen: TourScreen }

/** Os passos do tour do celular, na ordem; `anchor` é a peça que o balão aponta. */
export const MOBILE_TOUR: readonly TourStep[] = [
  { id: 'new', anchor: 'home-new', need: 'emptyList', screen: 'home' },
  { id: 'menu', anchor: 'session-row', need: 'sessions', screen: 'home' },
  { id: 'search', anchor: 'list-search', need: 'none', screen: 'home' },
  { id: 'attach', anchor: 'composer-attach', need: 'none', screen: 'chat' },
  { id: 'dictation', anchor: 'composer-mic', need: 'none', screen: 'chat' },
  { id: 'model', anchor: 'composer-model', need: 'none', screen: 'chat' },
];

/** Dicas próprias, fora do tour: cada uma aparece uma vez, quando a peça dela surge na tela. */
export const MOBILE_TIPS: readonly TourStep[] = [
  { id: 'media', anchor: 'chat-more', need: 'none', screen: 'chat' },
];

/** Dica nascida de um passo que não cabia na hora: id próprio para não se confundir com as dicas. */
export const stepTipId = (step: TourStep) => `tour-${step.id}`;

/** Peças que só existem na tela inicial em modo lista: no modo nova conversa (e no tablet) elas não aparecem. */
const LIST_ANCHORS = new Set(['home-new', 'session-row', 'list-search']);
export type TourHomeMode = 'sessions' | 'newConversation';

/**
 * Planeja o tour com o número de sessões lido e o modo da tela inicial. Passo sem a condição, ou
 * cuja peça não existe no modo atual, sai e vira dica, oferecida quando a peça aparecer; o
 * primeiro (lista vazia) sem ela só some.
 */
export function planMobileTour(sessions: number, home: TourHomeMode = 'sessions'): { steps: TourStep[]; tips: TourStep[] } {
  const steps: TourStep[] = [];
  const tips: TourStep[] = [];
  for (const step of MOBILE_TOUR) {
    const shown = home === 'sessions' || !LIST_ANCHORS.has(step.anchor);
    const holds = shown && (step.need === 'emptyList' ? sessions === 0 : step.need === 'sessions' ? sessions > 0 : true);
    if (holds) steps.push(step);
    else if (step.need !== 'emptyList') tips.push({ ...step, id: stepTipId(step) });
  }
  return { steps, tips: [...tips, ...MOBILE_TIPS] };
}

/** Uma volta do tour: o passo na tela e se algum foi feito pela ação (o fim grava `done`) ou só pulado. */
export interface TourRun { steps: TourStep[]; index: number | null; doneAny: boolean }

export type TourChoice = 'next' | 'skipTour' | 'later';

export const startTourRun = (steps: TourStep[]): TourRun => ({ steps, index: steps.length ? 0 : null, doneAny: false });

/** Fim da volta: `outcome` é o que gravar; `null` = ainda andando ou adiado. */
export interface RunStep { run: TourRun; outcome: TourOutcome | null }

function advance(run: TourRun, doneAny: boolean): RunStep {
  const next = run.index === null ? null : run.index + 1;
  if (next === null || next >= run.steps.length) return { run: { ...run, index: null, doneAny }, outcome: doneAny ? 'done' : 'skipped' };
  return { run: { ...run, index: next, doneAny }, outcome: null };
}

/** A pessoa fez a ação do passo: só o passo na tela anda. */
export function runAction(run: TourRun, stepId: string): RunStep {
  if (run.index === null || run.steps[run.index]?.id !== stepId) return { run, outcome: null };
  return advance(run, true);
}

export function runChoice(run: TourRun, choice: TourChoice): RunStep {
  if (run.index === null) return { run, outcome: null };
  if (choice === 'next') return advance(run, run.doneAny);
  if (choice === 'skipTour') return { run: { ...run, index: null }, outcome: run.doneAny ? 'done' : 'skipped' };
  return { run: { ...run, index: null }, outcome: null };
}

export const runStep = (run: TourRun): TourStep | null => (run.index === null ? null : run.steps[run.index] ?? null);

/**
 * A pessoa está na tela do passo (outra peça dela está à vista) e a peça dele não: ela não vai aparecer
 * (sessões ocultas, grupo recolhido, linha fora da rolagem). Fora da tela do passo, ele só espera.
 */
export function stepStalled(step: TourStep, onScreen: ReadonlySet<string>): boolean {
  if (onScreen.has(step.anchor)) return false;
  return screenVisible(step, onScreen);
}

/** Alguma outra peça da tela do passo está à vista: a pessoa está na tela dele. */
const screenVisible = (step: TourStep, onScreen: ReadonlySet<string>) =>
  [...MOBILE_TOUR, ...MOBILE_TIPS].some((other) => other.screen === step.screen && other.anchor !== step.anchor && onScreen.has(other.anchor));

/**
 * Passo travado na própria tela (`stepStalled`): sai da volta e vira dica que espera a peça; a volta
 * segue no próximo passo. Fora da tela dele nada muda: o passo da conversa espera a conversa abrir.
 */
export function runDropStep(run: TourRun, onScreen: ReadonlySet<string>): RunStep & { tip: TourStep | null } {
  const step = runStep(run);
  if (!step || run.index === null || !stepStalled(step, onScreen)) return { run, outcome: null, tip: null };
  const steps = run.steps.filter((_, i) => i !== run.index);
  const index = run.index < steps.length ? run.index : null;
  const outcome: TourOutcome | null = index === null ? (run.doneAny ? 'done' : 'skipped') : null;
  return { run: { steps, index, doneAny: run.doneAny }, outcome, tip: { ...step, id: stepTipId(step) } };
}

/** A dica a oferecer agora: a primeira cuja peça está na tela e que ainda não foi vista. */
export function nextTip(tips: readonly TourStep[], onScreen: ReadonlySet<string>, progress: TourProgress): TourStep | null {
  if (tourTipsOff(progress)) return null;
  return tips.find((tip) => onScreen.has(tip.anchor) && !tourTipSeen(progress, tip.id)) ?? null;
}

/** O que o balão mostra agora. */
export type TourTarget = { kind: 'step'; step: TourStep; n: number; total: number } | { kind: 'tip'; step: TourStep };

/**
 * Passo pendente com a peça na tela vence; fora da tela ele espera, e as dicas também, até
 * `stalled` (a peça sumiu por tempo demais): aí as dicas voltam a aparecer enquanto ele espera.
 */
export function tourTarget(run: TourRun | null, tips: readonly TourStep[], onScreen: ReadonlySet<string>, progress: TourProgress, stalled: boolean): TourTarget | null {
  const step = run ? runStep(run) : null;
  if (step && run?.index != null) {
    if (onScreen.has(step.anchor)) return { kind: 'step', step, n: run.index + 1, total: run.steps.length };
    // Na tela do passo ele segura as dicas até virar dica; noutra tela, as dicas dela aparecem enquanto ele espera.
    if (!stalled && screenVisible(step, onScreen)) return null;
  }
  const tip = nextTip(tips, onScreen, progress);
  return tip ? { kind: 'tip', step: tip } : null;
}

/** Lista de cada máquina do dono: só conta sessões quando todas responderam, fora as que estão em queda. */
export function tourListsReady(buckets: readonly { loaded: boolean; error: string | null; server: { invite?: unknown } }[]): boolean {
  const own = buckets.filter((b) => !b.server.invite);
  // Só convites: não há máquina própria para esperar; a primeira lista lida basta.
  if (own.length === 0) return buckets.some((b) => b.loaded);
  return own.some((b) => b.loaded) && own.every((b) => b.loaded || b.error !== null);
}

/** `GET`/`PUT /api/tour-progress`. Servidor sem a rota (404/405): `null`. */
export async function tourProgressFor(s: Server, patch?: TourProgress, fetcher?: typeof fetch): Promise<TourProgress | null> {
  const init: RequestInit | undefined = patch ? { method: 'PUT', body: JSON.stringify(patch) } : undefined;
  try {
    return await apiFetchForServer<TourProgress>(s, '/api/tour-progress', init, 15000, true, fetcher);
  } catch (e) {
    if (isMissingRoute(e)) return null;
    throw e;
  }
}
