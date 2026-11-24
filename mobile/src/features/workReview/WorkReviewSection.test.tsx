// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { WorkReview, WorkReviewResponse } from '@hangar/core';
import * as m from '../../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const api = vi.hoisted(() => ({ fetch: vi.fn(), send: vi.fn(), save: vi.fn() }));
const curtain = vi.hoisted(() => ({ last: null as null | { items: { title: string; leftTag: string; rightTag: string }[]; index: number } }));
vi.mock('./api', async (original) => ({
  ...(await original<typeof import('./api')>()),
  fetchWorkReview: api.fetch,
  sendWorkReviewDecisions: api.send,
  saveWorkReviewDraft: api.save,
  workReviewAssetSource: (_s: unknown, _n: string, review: string, asset: string) => ({ uri: `img:${review}:${asset}`, headers: {} }),
}));
vi.mock('../../stores/servers', () => {
  const state = { servers: [{ id: 'srv', label: 'Casa', baseUrl: 'https://casa.test', token: 't' }] };
  return { useServers: (select: (s: typeof state) => unknown) => select(state) };
});
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { erro: vi.fn(), ok: vi.fn() } }));
vi.mock('../../ui/MultilineInput', () => ({
  MultilineInput: (p: { value: string; onChangeText: (t: string) => void; accessibilityLabel?: string }) =>
    createElement('textarea', { value: p.value, 'aria-label': p.accessibilityLabel, onChange: (e: { target: { value: string } }) => p.onChangeText(e.target.value) }),
}));
vi.mock('./ZoomFrame', () => ({
  ZoomFrame: (p: { label: string; onPress: () => void }) => createElement('button', { 'aria-label': p.label, onClick: p.onPress }, 'frame'),
}));
vi.mock('./CompareCurtain', () => ({
  CompareCurtain: (p: typeof curtain.last) => { curtain.last = p; return createElement('div', { 'data-curtain': true }, p!.items[p!.index].title); },
}));

import { setWorkReviewSummaries, clearWorkReviewSummaries } from './summaries';
import { WorkReviewSection, hasWorkReview } from './WorkReviewSection';
import { useWorkReview } from './useWorkReview';

const waiting = (): WorkReview => ({
  id: 'r-7', title: 'Comparação com o mock', subject: 'Controle Bancário', updated_at: new Date().toISOString(), state: 'waiting',
  steps: { labels: ['Prints', 'Comparação', 'Sua decisão', 'Pronto'], current: 2 },
  headline: { value: '2', text: 'diferenças esperam a sua decisão', detail: 'O agente já corrigiu 15 sozinho.' },
  badge: { text: 'mock: 2 para você', tone: 'warning' },
  decisions: [
    { id: '47', tag: 'Diferença 47', title: 'Janela menor', reason: 'padrão do kit', reason_label: 'Por que não igualou',
      left: { label: 'Mock', asset: 'a1', width: 1600, height: 900, box: [385, 263, 830, 372] },
      right: { label: 'Tela', asset: 'a2', width: 1600, height: 900, box: [385, 263, 830, 372] },
      choices: [{ id: 'igualar', label: 'Igualar ao mock' }, { id: 'manter', label: 'Manter', requires_reason: true, default_reason: 'padrão' }] },
    { id: '86', title: 'Título da impressão',
      choices: [{ id: 'igualar', label: 'Igualar ao mock' }, { id: 'manter', label: 'Manter', requires_reason: true, default_reason: 'kit',
        reason_prompt: 'Por que manter?' }] },
  ],
  resolved: { label: 'Corrigidas pelo agente', count: 15, items: [{ title: 'Cabeçalho', before: 'a3', after: 'a4' }] },
  kept: { label: 'Mantidas com motivo', items: [{ title: 'Selo Open Finance', reason: 'dado sem fonte' }] },
  links: [{ label: 'Relatório completo', url: 'http://127.0.0.1:47613/c/x/', local: true }, { label: 'Ticket', url: 'https://help.test/PM-1' }],
});
const linked = (review: WorkReview | null, via: 'ticket' | 'branch' = 'ticket'): WorkReviewResponse => ({
  linked: true, via, connector: 'help', ticket_id: 't1', key: 'PM-1', supported: true, review, fetched_at: null,
});

let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  vi.useFakeTimers();
  api.fetch.mockReset();
  api.send.mockReset();
  api.save.mockReset();
  curtain.last = null;
  clearWorkReviewSummaries();
  container = document.createElement('div');
  root = createRoot(container);
});
afterEach(() => { act(() => root.unmount()); vi.useRealTimers(); });

const flush = async () => { await act(async () => { await Promise.resolve(); await Promise.resolve(); }); };
const render = async (standalone = false) => {
  await act(async () => root.render(createElement(WorkReviewSection, { serverId: 'srv', name: 'pm', standalone })));
  await flush();
};
const button = (text: string) => [...container.querySelectorAll('button')].find((b) => b.textContent?.includes(text) || b.getAttribute('aria-label') === text);
const click = async (text: string) => { await act(async () => button(text)!.click()); await flush(); };
// O React só vê a mudança pelo setter nativo do campo, não pela atribuição direta.
const type = async (area: HTMLTextAreaElement, value: string) => {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(area, value);
    area.dispatchEvent(new Event('input', { bubbles: true }));
  });
};

describe('WorkReviewSection', () => {
  it('sem vínculo a seção some no Contexto; a folha diz que não há revisão', async () => {
    api.fetch.mockResolvedValue({ linked: false });
    await render();
    expect(container.textContent).toBe('');
    await render(true);
    expect(container.textContent).toContain(m.work_review_empty());
  });

  it('falha ao abrir aparece com tentar de novo quando a lista anunciou a revisão', async () => {
    setWorkReviewSummaries('srv', { pm: { review_id: 'r-7', state: 'waiting', badge: null, updated_at: 'x' } });
    api.fetch.mockRejectedValueOnce(Object.assign(new Error('424: entre no Help'), { status: 424 }));
    await render();
    expect(container.textContent).toContain(m.work_review_load_error({ msg: 'entre no Help' }));
    api.fetch.mockResolvedValue(linked(waiting()));
    await click(m.native_retry());
    expect(container.textContent).toContain('Comparação com o mock');
  });

  it('decide uma por vez: escolher sem motivo avança, manter pede motivo e envia todas juntas', async () => {
    api.fetch.mockResolvedValue(linked(waiting(), 'branch'));
    await render();
    const text = () => container.textContent ?? '';
    expect(text()).toContain(m.work_review_found_by_branch());
    expect(text()).toContain('Janela menor');
    expect(text()).toContain(m.work_review_position({ index: 1, total: 2 }));
    expect(button(m.work_review_pending({ count: 2 }))!.disabled).toBe(true);

    await click('Igualar ao mock');
    await act(async () => { vi.advanceTimersByTime(250); });
    expect(text()).toContain('Título da impressão');
    expect(button(m.work_review_pending_one())).toBeTruthy();

    await click('Manter');
    const area = container.querySelector('textarea')!;
    expect(area.value).toBe('kit');
    expect(area.getAttribute('aria-label')).toBe('Por que manter?');
    await type(area, ' ');
    expect(text()).toContain(m.work_review_reason_required());
    expect(button(m.work_review_pending_one())!.disabled).toBe(true);
    await type(area, 'regra do kit');

    api.send.mockResolvedValue({ ok: true, review: { ...waiting(), state: 'applying', decisions: [] } });
    await click(m.work_review_send());
    expect(api.send).toHaveBeenCalledWith(expect.objectContaining({ id: 'srv' }), 'pm', {
      review_id: 'r-7', decisions: [{ id: '47', choice: 'igualar' }, { id: '86', choice: 'manter', reason: 'regra do kit' }],
    });
    // Depois do envio a revisão é relida.
    expect(api.fetch).toHaveBeenCalledTimes(2);
  });

  it('cada escolha é guardada no servidor e volta ao abrir, na primeira que falta', async () => {
    api.fetch.mockResolvedValue({ ...linked(waiting()), draft: { 47: { choice: 'igualar', reason: '' } } });
    await render();
    expect(container.textContent).toContain('Título da impressão');
    expect(button(m.work_review_pending_one())).toBeTruthy();
    await click('Manter');
    // Só a decisão que mudou: o servidor junta com o que já guardou.
    expect(api.save).toHaveBeenLastCalledWith(expect.objectContaining({ id: 'srv' }), 'pm',
      { review_id: 'r-7', choices: { 86: { choice: 'manter', reason: 'kit' } } });
    let answer!: () => void;
    api.save.mockReturnValueOnce(new Promise<void>((resolve) => { answer = resolve; }));
    await type(container.querySelector('textarea')!, 'regra');
    await type(container.querySelector('textarea')!, 'regra do kit');
    // Uma gravação por vez: a do meio é pulada, e a última sai quando a anterior termina.
    expect(api.save).toHaveBeenCalledTimes(2);
    await act(async () => answer());
    await flush();
    expect(api.save).toHaveBeenCalledTimes(3);
    expect(api.save).toHaveBeenLastCalledWith(expect.anything(), 'pm',
      { review_id: 'r-7', choices: { 86: { choice: 'manter', reason: 'regra do kit' } } });
  });

  it('voltar antes de a gravação terminar mantém a escolha; falha de rede tenta de novo', async () => {
    api.fetch.mockResolvedValue(linked(waiting()));
    let fail!: (e: unknown) => void;
    api.save.mockReturnValueOnce(new Promise<void>((_, reject) => { fail = reject; }));
    await render();
    await click('Igualar ao mock');
    await act(async () => root.render(createElement(WorkReviewSection, { serverId: 'srv', name: 'outra' })));
    await flush();
    await act(async () => root.render(createElement(WorkReviewSection, { serverId: 'srv', name: 'pm' })));
    await flush();
    expect(button(m.work_review_pending_one())).toBeTruthy();
    await act(async () => fail(Object.assign(new Error('network'), { code: 'network_error' })));
    await flush();
    expect(api.save).toHaveBeenCalledTimes(1);
    await act(async () => { vi.advanceTimersByTime(5_000); });
    await flush();
    expect(api.save).toHaveBeenCalledTimes(2);
    expect(api.save).toHaveBeenLastCalledWith(expect.anything(), 'pm', { review_id: 'r-7', choices: { 47: { choice: 'igualar', reason: '' } } });
  });

  it('leitura começada antes da gravação confirmar é refeita, e a escolha não some', async () => {
    api.fetch.mockResolvedValue(linked(waiting()));
    let saved!: () => void;
    api.save.mockReturnValueOnce(new Promise<void>((resolve) => { saved = resolve; }));
    await render();
    await click('Igualar ao mock');
    await act(async () => root.render(createElement(WorkReviewSection, { serverId: 'srv', name: 'outra' })));
    await flush();
    let stale!: (v: unknown) => void;
    api.fetch.mockReturnValueOnce(new Promise((resolve) => { stale = resolve; }));
    api.fetch.mockResolvedValue({ ...linked(waiting()), draft: { 47: { choice: 'igualar', reason: '' } } });
    await act(async () => root.render(createElement(WorkReviewSection, { serverId: 'srv', name: 'pm' })));
    await flush();
    const reads = api.fetch.mock.calls.length;
    await act(async () => saved());
    await flush();
    await act(async () => stale(linked(waiting())));
    await flush();
    expect(api.fetch.mock.calls.length).toBe(reads + 1);
    expect(button(m.work_review_pending_one())).toBeTruthy();
  });

  it('gravação recusada guarda a escolha para a próxima', async () => {
    api.fetch.mockResolvedValue(linked(waiting()));
    api.save.mockRejectedValueOnce(Object.assign(new Error('400: x'), { status: 400 }));
    await render();
    await click('Igualar ao mock');
    await flush();
    await act(async () => { vi.advanceTimersByTime(250); });
    await click('Igualar ao mock');
    await flush();
    expect(api.save).toHaveBeenLastCalledWith(expect.anything(), 'pm',
      { review_id: 'r-7', choices: { 47: { choice: 'igualar', reason: '' }, 86: { choice: 'igualar', reason: '' } } });
  });

  it('com a revisão trocada o envio diz que mudou e relê', async () => {
    const review = waiting();
    review.decisions = [review.decisions[0]];
    api.fetch.mockResolvedValue(linked(review));
    await render();
    await click('Igualar ao mock');
    api.send.mockRejectedValue(Object.assign(new Error('409: stale'), { status: 409, code: 'stale_review' }));
    await click(m.work_review_send());
    expect(container.textContent).toContain(m.work_review_stale());
    expect(api.fetch).toHaveBeenCalledTimes(2);
  });

  it('a faixa usa o texto do conector só com o número dele; depois de escolher, o do Hangar no número certo', async () => {
    const review = waiting();
    review.headline = { value: '3', text: 'diferenças esperam a sua decisão' };
    review.decisions = [review.decisions[0], { ...review.decisions[0], id: '48', title: 'Outra janela' }, review.decisions[1]];
    api.fetch.mockResolvedValue(linked(review));
    await render();
    expect(container.textContent).toContain('3diferenças esperam a sua decisão');
    await click('Igualar ao mock');
    expect(container.textContent).toContain(`2${m.work_review_waiting()}`);
    expect(container.textContent).not.toContain('diferenças esperam a sua decisão');
    await act(async () => { vi.advanceTimersByTime(250); });
    await click('Igualar ao mock');
    expect(container.textContent).toContain(`1${m.work_review_waiting_one()}`);
  });

  it('com zero pendentes a faixa diz que as decisões estão prontas', async () => {
    const review = waiting();
    review.decisions = [review.decisions[0]];
    api.fetch.mockResolvedValue(linked(review));
    await render();
    await click('Igualar ao mock');
    expect(container.textContent).toContain(m.work_review_ready());
    expect(container.textContent).toContain(m.work_review_send());
  });

  it('o quadro abre a cortina com os rótulos dos lados; a tira abre a de antes e depois', async () => {
    api.fetch.mockResolvedValue(linked(waiting()));
    await render();
    await click(m.work_review_compare_title({ title: 'Janela menor', left: 'Mock', right: 'Tela' }));
    expect(curtain.last?.items[0]).toMatchObject({ leftTag: 'Mock', rightTag: 'Tela' });
    await click('Cabeçalho');
    expect(curtain.last?.items[0]).toMatchObject({ title: 'Cabeçalho', leftTag: m.work_review_before(), rightTag: m.work_review_after() });
    // 15 corrigidas, uma na tira: o resto vira "+14".
    expect(container.textContent).toContain(m.work_review_more({ count: 14 }));
  });

  it('link local fica só com o aviso; o outro é link', async () => {
    api.fetch.mockResolvedValue(linked(waiting()));
    await render();
    expect(container.textContent).toContain(m.work_review_local_link());
    expect(button('Relatório completo')).toBeUndefined();
    expect(container.querySelector('[role="link"]')?.textContent).toBe('Ticket');
  });

  it('rodando mostra o progresso e esconde as decisões e as corrigidas', async () => {
    api.fetch.mockResolvedValue(linked({ ...waiting(), state: 'running', decisions: [],
      progress: { label: 'Capturando os estados', done: 9, total: 16 } }));
    await render();
    expect(container.textContent).toContain('Capturando os estados');
    expect(container.textContent).toContain(m.work_review_position({ index: 9, total: 16 }));
    expect(container.textContent).not.toContain('Corrigidas pelo agente');
    expect(container.querySelector('[role="progressbar"]')).toBeTruthy();
  });

  it('trocar de sessão sem remontar não mostra a revisão da anterior', async () => {
    api.fetch.mockResolvedValue(linked(waiting()));
    await render();
    expect(container.textContent).toContain('Comparação com o mock');
    api.fetch.mockResolvedValue({ linked: false });
    await act(async () => root.render(createElement(WorkReviewSection, { serverId: 'srv', name: 'outra' })));
    await flush();
    expect(api.fetch).toHaveBeenLastCalledWith(expect.objectContaining({ id: 'srv' }), 'outra');
    expect(container.textContent).toBe('');
  });

  it('resposta do envio que chega depois de trocar de sessão não cai na nova nem relê a anterior', async () => {
    const review = waiting();
    review.decisions = [review.decisions[0]];
    api.fetch.mockResolvedValue(linked(review));
    await render();
    await click('Igualar ao mock');
    let answer!: (v: unknown) => void;
    api.send.mockReturnValue(new Promise((resolve) => { answer = resolve; }));
    await click(m.work_review_send());
    expect(api.send).toHaveBeenCalledWith(expect.anything(), 'pm', expect.anything());

    const other = waiting();
    other.id = 'r-9';
    other.title = 'Revisão da outra';
    // A releitura da anterior nunca responde: só a resposta do envio poderia trocar a tela.
    api.fetch.mockImplementation((_s: unknown, n: string) => (n === 'outra' ? Promise.resolve(linked(other)) : new Promise(() => {})));
    await act(async () => root.render(createElement(WorkReviewSection, { serverId: 'srv', name: 'outra' })));
    await flush();
    expect(container.textContent).toContain('Revisão da outra');
    const reads = api.fetch.mock.calls.length;

    await act(async () => answer({ ok: true, review: { ...review, title: 'Rodada da pm depois do envio' } }));
    await flush();
    expect(container.textContent).toContain('Revisão da outra');
    expect(container.textContent).not.toContain('Rodada da pm depois do envio');
    expect(api.fetch).toHaveBeenCalledTimes(reads);
    // O "enviando" da anterior não prende o botão da nova.
    await click('Igualar ao mock');
    await act(async () => { vi.advanceTimersByTime(250); });
    await click('Igualar ao mock');
    expect(button(m.work_review_send())!.disabled).toBe(false);
  });

  it('desligada não lê; ligada lê na hora', async () => {
    api.fetch.mockResolvedValue(linked(waiting()));
    const Probe = ({ enabled }: { enabled: boolean }) => { useWorkReview('srv', 'pm', enabled); return null; };
    await act(async () => root.render(createElement(Probe, { enabled: false })));
    await flush();
    expect(api.fetch).not.toHaveBeenCalled();
    await act(async () => root.render(createElement(Probe, { enabled: true })));
    await flush();
    expect(api.fetch).toHaveBeenCalledTimes(1);
  });

  it('aba Revisão: a leitura concluída vence a lista, e a releitura em voo mantém o último resultado', async () => {
    const shown: boolean[] = [];
    const Probe = () => { shown.push(hasWorkReview(useWorkReview('srv', 'pm'))); return null; };
    const deferred = () => { let resolve!: (v: WorkReviewResponse) => void; const p = new Promise<WorkReviewResponse>((r) => { resolve = r; }); return { p, resolve }; };
    const last = () => shown[shown.length - 1];

    setWorkReviewSummaries('srv', { pm: { review_id: 'r-7', state: 'waiting', badge: null, updated_at: 'x' } });
    const first = deferred();
    api.fetch.mockReturnValueOnce(first.p);
    await act(async () => root.render(createElement(Probe)));
    await flush();
    expect(last()).toBe(true);

    // A lista ainda anuncia, mas a leitura disse que não há revisão.
    await act(async () => first.resolve(linked(null)));
    await flush();
    expect(last()).toBe(false);

    const second = deferred();
    api.fetch.mockReturnValueOnce(second.p);
    const before = shown.length;
    await act(async () => setWorkReviewSummaries('srv', { pm: { review_id: 'r-8', state: 'waiting', badge: null, updated_at: 'y' } }));
    await flush();
    expect(api.fetch).toHaveBeenCalledTimes(2);
    expect(shown.slice(before).every((v) => !v)).toBe(true);
    await act(async () => second.resolve(linked(waiting())));
    await flush();
    expect(last()).toBe(true);

    api.fetch.mockResolvedValueOnce({ linked: false });
    await act(async () => setWorkReviewSummaries('srv', { pm: { review_id: 'r-8', state: 'done', badge: null, updated_at: 'z' } }));
    await flush();
    expect(last()).toBe(false);
  });

  it('o evento da lista com outra rodada relê a revisão', async () => {
    api.fetch.mockResolvedValue(linked(waiting()));
    await render();
    expect(api.fetch).toHaveBeenCalledTimes(1);
    await act(async () => setWorkReviewSummaries('srv', { pm: { review_id: 'r-8', state: 'running', badge: null, updated_at: 'y' } }));
    await flush();
    expect(api.fetch).toHaveBeenCalledTimes(2);
  });
});
