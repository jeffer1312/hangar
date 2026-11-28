import { beforeEach, describe, expect, it } from 'vitest';
import { configureApi, type EventSourceLike } from './apiEnv';
import { _limparEsfriamentoParaTestes } from './esfriamento';
import {
  MOBILE_TOUR_DONE, MOBILE_TOUR_SKIPPED, mergeTourProgress, mobileTourOutcome, nextTip, planMobileTour, runAction,
  runChoice, runStep, sameTourProgress, seenTip, seenTour, startTourRun, tipsSwitch, tourProgressFor, tourTipsOff,
} from './tour';

describe('junção do visto', () => {
  it('done vence skipped e nada perde para os dois', () => {
    expect(mergeTourProgress({ tour: 'done' }, { tour: 'skipped' }).tour).toBe('done');
    expect(mergeTourProgress({ tour: 'skipped' }, { tour: 'done' }).tour).toBe('done');
    expect(mergeTourProgress({}, { tour: 'skipped' }).tour).toBe('skipped');
    expect(mergeTourProgress({ tour: 'skipped' }, {}).tour).toBe('skipped');
  });

  it('as dicas somam e o interruptor mais novo vence pelo instante, não pelo texto', () => {
    const ours = { tips: ['b'], tips_off: { on: true, at: '2026-10-30T12:00:00Z' } };
    const merged = mergeTourProgress(ours, { tips: ['a', 'b'], tips_off: { on: false, at: '2026-10-30T11:00:00Z' } });
    expect(merged.tips).toEqual(['a', 'b']);
    expect(merged.tips_off).toEqual({ on: true, at: '2026-10-30T12:00:00Z' });
    expect(mergeTourProgress(merged, { tips_off: { on: false, at: '2026-10-30T10:00:01-02:00' } }).tips_off)
      .toEqual({ on: false, at: '2026-10-30T10:00:01-02:00' });
  });

  it('o tour do celular mora nas dicas e não mexe no tour do PC', () => {
    expect(mobileTourOutcome({ tour: 'done', tips: [] })).toBeNull();
    expect(mobileTourOutcome(seenTour('skipped'))).toBe('skipped');
    expect(mobileTourOutcome(mergeTourProgress(seenTour('skipped'), seenTour('done')))).toBe('done');
    expect(seenTour('done')).toEqual({ tips: [MOBILE_TOUR_DONE] });
    expect(seenTour('skipped')).toEqual({ tips: [MOBILE_TOUR_SKIPPED] });
  });

  it('igualdade ignora a ordem das dicas', () => {
    expect(sameTourProgress({ tips: ['b', 'a'] }, { tips: ['a', 'b'], tour: null })).toBe(true);
    expect(sameTourProgress({ tips: ['a'] }, { tips: ['a', 'c'] })).toBe(false);
  });

  it('o interruptor das dicas leva a hora', () => {
    const p = tipsSwitch(true, new Date('2026-11-28T12:00:00Z'));
    expect(p).toEqual({ tips_off: { on: true, at: '2026-11-28T12:00:00.000Z' } });
    expect(tourTipsOff(p)).toBe(true);
  });
});

describe('plano do tour do celular', () => {
  it('lista vazia começa pela nova conversa e guarda o menu da sessão como dica', () => {
    const { steps, tips } = planMobileTour(0);
    expect(steps.map((s) => s.id)).toEqual(['new', 'search', 'attach', 'dictation', 'model']);
    expect(tips.map((t) => t.id)).toEqual(['tour-menu', 'media']);
  });

  it('com sessões o primeiro passo some sem virar dica', () => {
    const { steps, tips } = planMobileTour(3);
    expect(steps.map((s) => s.id)).toEqual(['menu', 'search', 'attach', 'dictation', 'model']);
    expect(tips.map((t) => t.id)).toEqual(['media']);
  });
});

describe('volta do tour', () => {
  const steps = planMobileTour(2).steps;

  it('só a ação do passo na tela anda, e o fim com ação grava done', () => {
    let run = startTourRun(steps.slice(0, 2));
    expect(runAction(run, 'search').run.index).toBe(0);
    let step = runAction(run, 'menu');
    expect(step.outcome).toBeNull();
    run = step.run;
    expect(runStep(run)?.id).toBe('search');
    step = runAction(run, 'search');
    expect(step.outcome).toBe('done');
    expect(runStep(step.run)).toBeNull();
  });

  it('pular cada passo até o fim grava skipped', () => {
    let step = runChoice(startTourRun(steps.slice(0, 2)), 'next');
    expect(step.outcome).toBeNull();
    step = runChoice(step.run, 'next');
    expect(step.outcome).toBe('skipped');
  });

  it('pular o tour depois de uma ação feita grava done; ver depois não grava nada', () => {
    const afterAction = runAction(startTourRun(steps), 'menu').run;
    expect(runChoice(afterAction, 'skipTour').outcome).toBe('done');
    expect(runChoice(startTourRun(steps), 'skipTour').outcome).toBe('skipped');
    const later = runChoice(startTourRun(steps), 'later');
    expect(later.outcome).toBeNull();
    expect(runStep(later.run)).toBeNull();
  });
});

describe('dicas', () => {
  const { tips } = planMobileTour(0);

  it('oferece a primeira dica com a peça na tela e ainda não vista', () => {
    expect(nextTip(tips, new Set(['chat-more']), {})?.id).toBe('media');
    expect(nextTip(tips, new Set(['session-row', 'chat-more']), {})?.id).toBe('tour-menu');
    expect(nextTip(tips, new Set(['chat-more']), seenTip('media'))).toBeNull();
    expect(nextTip(tips, new Set(), {})).toBeNull();
  });

  it('dicas desligadas não aparecem', () => {
    expect(nextTip(tips, new Set(['chat-more']), tipsSwitch(true))).toBeNull();
    expect(nextTip(tips, new Set(['chat-more']), mergeTourProgress(tipsSwitch(true, new Date(1)), tipsSwitch(false, new Date(2))))?.id).toBe('media');
  });
});

describe('rota', () => {
  const server = { id: 's', label: 'casa', baseUrl: 'http://casa.test', token: 't' };
  beforeEach(() => {
    _limparEsfriamentoParaTestes();
    configureApi({ getBaseUrl: () => 'http://casa.test', getToken: () => 't', onUnauthorized: () => {}, origin: 'http://app.test',
      createEventSource: () => ({ addEventListener() {}, removeEventListener() {}, close() {}, onerror: null, onopen: null, readyState: 0 }) as unknown as EventSourceLike });
  });

  it('lê e grava pelo mesmo caminho, mandando só o que acrescentar', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    const fetcher = (async (url: string, init?: RequestInit) => {
      calls.push({ url: String(url), init });
      return new Response(JSON.stringify({ tour: null, tips: ['mobile-tip:media'] }), { status: 200, headers: { 'content-type': 'application/json' } });
    }) as unknown as typeof fetch;
    expect(await tourProgressFor(server as never, undefined, fetcher)).toEqual({ tour: null, tips: ['mobile-tip:media'] });
    await tourProgressFor(server as never, seenTip('media'), fetcher);
    expect(calls.map((c) => c.url.endsWith('/api/tour-progress'))).toEqual([true, true]);
    expect(calls[1].init?.method).toBe('PUT');
    expect(JSON.parse(String(calls[1].init?.body))).toEqual({ tips: ['mobile-tip:media'] });
  });

  it('servidor sem a rota devolve null', async () => {
    const fetcher = (async () => new Response('{"detail":"Not Found"}', { status: 404 })) as unknown as typeof fetch;
    expect(await tourProgressFor(server as never, undefined, fetcher)).toBeNull();
  });
});
