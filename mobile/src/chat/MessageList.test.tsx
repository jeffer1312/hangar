// @vitest-environment happy-dom
import { act, createElement, useImperativeHandle, type ComponentProps, type ReactNode, type Ref } from 'react';
import type { NativeScrollEvent, NativeSyntheticEvent } from 'react-native';
import type { LegendListRef } from '@legendapp/list/react-native';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, configureLocale, type ChatEvent, type OrqEntry, type SessionInfo } from '@hangar/core';
import { overwriteGetLocale } from '../paraglide/runtime';
import { MessageList } from './MessageList';
import { chatStore, _resetChatsForTests } from '../stores/chat';
import * as m from '../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

overwriteGetLocale(() => 'pt');
configureLocale({ getLocale: () => 'pt' });
vi.mock('../paraglide/messages', async (original) => ({
  ...await original<typeof import('../paraglide/messages')>(),
  ...Object.fromEntries(
    ('native_delivery_failed native_delivery_unproved native_queue_held native_no_safe_delivery native_delivering native_queued '
      + 'native_queue_discard msg_nao_chegou_reenvie msg_aria_mensagens chat_servidor_removido')
      .split(' ').map((key) => [key, () => key]),
  ),
  native_queue_discard_message: ({ text }: { text: string }) => `native_queue_discard_message:${text}`,
}));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {}, remove: () => {} }) }));
const servers = vi.hoisted(() => ({ servers: [
  { id: 'a', label: 'a', baseUrl: 'http://10.0.0.1:8765', token: 'token-a' },
  { id: 'b', label: 'b', baseUrl: 'http://10.0.0.2:8765', token: 'token-b' },
] }));
vi.mock('../stores/servers', () => ({ useServers: { getState: () => servers } }));
const appearance = vi.hoisted(() => ({ ferramentas: undefined as string | undefined }));
vi.mock('../stores/aparencia', () => ({ useAparencia: (select: (s: unknown) => unknown) => select({ pensamentoTools: 'busca', ferramentas: appearance.ferramentas }) }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
// Só a casca visual é substituída; lista, notas, ação, store e API da fila são os reais.
vi.mock('./UserBubble', () => ({ UserBubble: ({ text }: { text: string }) => createElement('span', { 'data-user': true }, text) }));
vi.mock('./AssistantBubble', () => ({ AssistantBubble: () => null, mkMarkdownStyle: () => ({}) }));
vi.mock('react-native-enriched-markdown', () => ({ EnrichedMarkdownText: ({ markdown }: { markdown: string }) => createElement('article', null, markdown) }));
vi.mock('../ui/Sheet', () => ({ Sheet: ({ children }: { children: ReactNode }) => createElement('section', null, children) }));
vi.mock('../ui/Toast', () => ({ toast: { erro: vi.fn() } }));
vi.mock('./PreviewBubble', () => ({ PreviewBubble: () => null }));
vi.mock('./LiveWork', () => ({ AgentesRodando: () => null, PensamentoVivo: () => null, WorkingLine: () => null }));
vi.mock('./TaskList', () => ({ TaskList: () => null }));
vi.mock('./tools/ToolCard', () => ({ ToolCard: () => null }));
vi.mock('./tools/ToolDetailSheet', () => ({ ToolDetailSheet: () => null }));
vi.mock('./tools/HtmlPageCard', () => ({ HtmlPageCard: () => null }));
type ListHandle = Pick<LegendListRef, 'scrollToEnd'> & {
  getState: () => Partial<ReturnType<LegendListRef['getState']>>;
};
type ListProps = {
  ref?: Ref<ListHandle>;
  data: { id: string }[];
  renderItem: (row: { item: unknown }) => ReactNode;
  ListFooterComponent?: ReactNode;
  alignItemsAtEnd?: boolean;
  maintainScrollAtEnd?: boolean;
  initialScrollAtEnd?: boolean;
  maintainVisibleContentPosition?: boolean;
  nestedScrollEnabled?: boolean;
  recycleItems?: boolean;
  onScroll: (event: NativeSyntheticEvent<NativeScrollEvent>) => void;
  onContentSizeChange?: (width: number, height: number) => void;
  onStartReached?: () => void;
};
const list = vi.hoisted(() => ({ render: vi.fn(), data: vi.fn(), alignment: vi.fn(), props: vi.fn<(props: ListProps) => void>(),
  scrollToEnd: vi.fn<LegendListRef['scrollToEnd']>(), geometry: {} as Partial<ReturnType<LegendListRef['getState']>> }));
vi.mock('@legendapp/list/react-native', () => ({
  LegendList: (props: ListProps) => {
    const { data, renderItem, ListFooterComponent, alignItemsAtEnd, ref } = props;
    useImperativeHandle(ref, () => ({ scrollToEnd: list.scrollToEnd, getState: () => list.geometry }));
    list.props(props);
    list.render(renderItem);
    list.data(data);
    list.alignment(alignItemsAtEnd);
    return createElement('div', null,
      ...data.map((item) => createElement('div', { key: item.id, 'data-row': item.id }, renderItem({ item }))), ListFooterComponent);
  },
}));

type Props = ComponentProps<typeof MessageList>;
const session = { name: 'sess', provider: 'claude', headless: false } as SessionInfo;
const event = (patch: Partial<ChatEvent> = {}): ChatEvent => ({ id: 'queued-one', kind: 'user_msg', text: 'mensagem', ...patch });
const props = (events: ChatEvent[], extra: Partial<Props> = {}): Props => ({
  events, preview: '', olderFailed: '', onLoadOlder: () => {}, session,
  stateEvent: { session: 'sess', state: 'working' }, ...extra,
});
const roots: Root[] = [];
function mount(node: ReactNode) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  act(() => root.render(node));
  return { container, root };
}
const discardButtons = (container: HTMLElement) => [...container.querySelectorAll<HTMLButtonElement>('[aria-label^="native_queue_discard_message:"]')];

beforeEach(() => {
  appearance.ferramentas = undefined;
  list.render.mockClear();
  list.data.mockClear();
  list.alignment.mockClear();
  list.props.mockClear();
  list.scrollToEnd.mockClear();
  list.geometry = {};
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{"ok":true}', { status: 200 })));
  // B está ativo, mas o chat abaixo é de A.
  configureApi({ getBaseUrl: () => servers.servers[1].baseUrl, getToken: () => 'token-b', onUnauthorized: () => {}, origin: null,
    createEventSource: () => { throw new Error('unexpected SSE'); } });
});
afterEach(() => {
  act(() => { for (const root of roots.splice(0)) root.unmount(); });
  _resetChatsForTests();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const groupEvents = (count = 2): ChatEvent[] => Array.from({ length: count }, (_, i) => ({ id: `thought-${i}`, kind: 'thinking', text: `thought-${i}-text` }));
const latestList = () => list.props.mock.lastCall![0];
const expandGroup = (container: HTMLElement) => act(() => container.querySelector<HTMLButtonElement>('[data-row] button')!.click());

it('Terminal vai ao fim sem animação e pausa o acompanhamento ao expandir', () => {
  appearance.ferramentas = 'terminal';
  const events: ChatEvent[] = [
    { id: 'read', kind: 'tool_use', tool_use_id: 'read', tool_name: 'Read', tool_input: { file_path: '/src/a.ts' } },
    { id: 'result', kind: 'tool_result', tool_use_id: 'read', result: 'ok' },
  ];
  const { container } = mount(createElement(MessageList, props(events)));
  act(() => latestList().onScroll({ nativeEvent: {
    contentOffset: { y: 0 }, contentSize: { height: 3000 }, layoutMeasurement: { height: 500 },
  } } as NativeSyntheticEvent<NativeScrollEvent>));
  const bottom = [...container.querySelectorAll('button')].find((button) => button.getAttribute('aria-label') === m.msg_ir_ultima())!;
  act(() => bottom.click());
  expect(list.scrollToEnd).toHaveBeenLastCalledWith({ animated: false });
  expandGroup(container);
  expect(latestList().maintainScrollAtEnd).toBe(false);
  expect(latestList().onContentSizeChange).toBeTypeOf('function');
});

it('Terminal junta a sequência desde a primeira chamada e preserva a saída aberta no append', () => {
  appearance.ferramentas = 'terminal';
  const read = (id: string): ChatEvent => ({ id, kind: 'tool_use', tool_use_id: id, tool_name: 'Read', tool_input: { file_path: `/src/${id}.ts` } });
  const done = (id: string): ChatEvent => ({ id: `r-${id}`, kind: 'tool_result', tool_use_id: id, result: 'ok\nexpanded-read-output' });
  const extra = { eventsRevision: 0, session: { ...session, plugin_tool_servers: null } };
  const events = [read('one'), done('one')];
  const { container, root } = mount(createElement(MessageList, props(events, extra)));
  const row = container.querySelector('[data-row]');
  expandGroup(container);
  const head = [...container.querySelectorAll('button')].find((button) => button.getAttribute('aria-label')?.startsWith('Read:'))!;
  act(() => head.click());
  expect(container.textContent).toContain('expanded-read-output');
  const next = [...events, read('two'), done('two')];
  act(() => root.render(createElement(MessageList, props(next, extra))));
  expect(container.querySelectorAll('[data-row]')).toHaveLength(1);
  expect(container.querySelector('[data-row]')).toBe(row);
  act(() => root.render(createElement(MessageList, props([...next, read('three')], extra))));
  expect(container.querySelector('[data-row]')).toBe(row);
  expect(container.textContent).toContain('expanded-read-output');
});

it('expansão manual pausa todos os gatilhos e resiste a clone, append e callbacks agendados', () => {
  vi.useFakeTimers();
  const frames: FrameRequestCallback[] = [];
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.push(callback); return frames.length; });
  vi.stubGlobal('cancelAnimationFrame', vi.fn());
  const initial = props(groupEvents(), { preserveToolGroupPosition: true });
  const { container, root } = mount(createElement(MessageList, initial));
  const renderItem = latestList().renderItem;
  expect(latestList().maintainScrollAtEnd).toBe(true);
  expect(frames).toHaveLength(1);
  expandGroup(container);
  expect(latestList().maintainScrollAtEnd).toBe(false);
  expect(latestList().renderItem).toBe(renderItem);
  expect(container.textContent).toContain('thought-0-text');
  act(() => { frames.forEach(frame => frame(0)); vi.advanceTimersByTime(300); });
  expect(list.scrollToEnd).not.toHaveBeenCalled();
  const cloned: ChatEvent[] = JSON.parse(JSON.stringify(initial.events));
  act(() => root.render(createElement(MessageList, { ...initial, events: cloned })));
  expect(latestList().maintainScrollAtEnd).toBe(false);
  act(() => root.render(createElement(MessageList, { ...initial, events: [...cloned, event({ id: 'new-row' })], bottomInset: 40 })));
  act(() => { frames.forEach(frame => frame(0)); vi.advanceTimersByTime(300); });
  expect(latestList().maintainScrollAtEnd).toBe(false);
  expect(list.scrollToEnd).not.toHaveBeenCalled();
});

const scrollList = (y: number, height: number, viewport: number) => act(() => latestList().onScroll({
  nativeEvent: { contentOffset: { y }, contentSize: { height }, layoutMeasurement: { height: viewport } },
} as NativeSyntheticEvent<NativeScrollEvent>));

it('scroll atrasado não desfaz pausa antes nem depois da nova medida', () => {
  vi.useFakeTimers();
  list.geometry = { contentLength: 400, scroll: 0, scrollLength: 400 };
  const initial = props(groupEvents(), { preserveToolGroupPosition: true });
  const { container, root } = mount(createElement(MessageList, initial));
  expandGroup(container);
  scrollList(0, 400, 400); // Evento anterior à expansão, ainda sem nova medida.
  expect(latestList().maintainScrollAtEnd).toBe(false);
  act(() => latestList().onContentSizeChange?.(390, 2000));
  scrollList(0, 400, 400); // O estado da lista também ainda não acompanhou a medida.
  expect(latestList().maintainScrollAtEnd).toBe(false);
  list.geometry = {};
  scrollList(0, 400, 400);
  expect(latestList().maintainScrollAtEnd).toBe(false);
  list.geometry = { contentLength: 2000, scroll: 0, scrollLength: 400 };
  scrollList(0, 400, 400); // Agora só o evento está atrasado.
  expect(latestList().maintainScrollAtEnd).toBe(false);
  act(() => root.render(createElement(MessageList, { ...initial, events: [...initial.events, event({ id: 'new-row' })] })));
  act(() => vi.advanceTimersByTime(300));
  expect(list.scrollToEnd).not.toHaveBeenCalled();
  list.geometry = { contentLength: 2000, scroll: 1600, scrollLength: 400 };
  scrollList(1600, 2000, 400);
  expect(latestList().maintainScrollAtEnd).toBe(true);
  expandGroup(container);
  list.geometry = { contentLength: 2000, scroll: 0, scrollLength: 400 };
  expandGroup(container);
  act(() => latestList().onContentSizeChange?.(390, 2000));
  expect(latestList().maintainScrollAtEnd).toBe(false);
  list.geometry = { contentLength: 2000, scroll: 1600, scrollLength: 400 };
  scrollList(1600, 2000, 400); // Altura igual à anterior, mas confirmada pelo callback.
  expect(latestList().maintainScrollAtEnd).toBe(true);
});

it('retoma só no fim medido pelo scroll ou pelo botão Ir pro fim', () => {
  vi.useFakeTimers();
  list.geometry = { contentLength: 400, scroll: 0, scrollLength: 400 };
  const initial = props(groupEvents(), { preserveToolGroupPosition: true });
  const { container, root } = mount(createElement(MessageList, initial));
  expandGroup(container);
  list.geometry = { contentLength: 2000, scroll: 0, scrollLength: 400 };
  scrollList(0, 2000, 400);
  expect(latestList().maintainScrollAtEnd).toBe(false);
  list.geometry = { contentLength: 2000, scroll: 1600, scrollLength: 400 };
  scrollList(1600, 2000, 400);
  expect(latestList().maintainScrollAtEnd).toBe(true);
  act(() => root.render(createElement(MessageList, { ...initial, events: [...initial.events, event({ id: 'new-row' })] })));
  act(() => vi.advanceTimersByTime(300));
  expect(list.scrollToEnd).toHaveBeenCalledWith({ animated: false });
  expandGroup(container); // Recolhe sem pausar.
  expect(latestList().maintainScrollAtEnd).toBe(true);
  expandGroup(container);
  list.geometry = { contentLength: 2400, scroll: 0, scrollLength: 400 };
  scrollList(0, 2400, 400);
  expect(latestList().maintainScrollAtEnd).toBe(false);
  list.scrollToEnd.mockClear();
  act(() => container.querySelector<HTMLButtonElement>(`[aria-label="${m.msg_ir_ultima()}"]`)!.click());
  expect(latestList().maintainScrollAtEnd).toBe(true);
  expect(list.scrollToEnd).toHaveBeenCalledWith({ animated: true });
});

it('medida real libera expansão que coube e Mostrar todas pausa novamente', () => {
  vi.useFakeTimers();
  const { container } = mount(createElement(MessageList, props(groupEvents(41), { preserveToolGroupPosition: true })));
  expandGroup(container);
  const measure = (height: number) => act(() => latestList().onContentSizeChange?.(390, height));
  measure(430); // Sem medida da viewport, não presume zero.
  expect(latestList().maintainScrollAtEnd).toBe(false);
  list.geometry = { scroll: 0, scrollLength: 0, isWithinMaintainScrollAtEndThreshold: true };
  measure(430);
  expect(latestList().maintainScrollAtEnd).toBe(false);
  list.geometry = { scroll: 0, scrollLength: 400, isWithinMaintainScrollAtEndThreshold: true };
  measure(2000); // O sinal guardado de proximidade não decide a retomada.
  expect(latestList().maintainScrollAtEnd).toBe(false);
  list.geometry = { scrollLength: 400 };
  measure(430);
  expect(latestList().maintainScrollAtEnd).toBe(false);
  list.geometry = { scroll: 0, scrollLength: 400, isWithinMaintainScrollAtEndThreshold: false };
  measure(Number.NaN);
  expect(latestList().maintainScrollAtEnd).toBe(false);
  measure(430);
  expect(latestList().maintainScrollAtEnd).toBe(true);
  const showAll = [...container.querySelectorAll('button')].find(button => button.textContent === m.tool_mostrar_todas({ n: 41 }))!;
  act(() => showAll.click());
  expect(latestList().maintainScrollAtEnd).toBe(false);
  expect(container.textContent).toContain('thought-40-text');
  measure(2100);
  act(() => vi.advanceTimersByTime(300));
  expect(latestList().maintainScrollAtEnd).toBe(false);
  expect(list.scrollToEnd).not.toHaveBeenCalled();
});

it('defaults preservam acompanhamento e virtualização; nested scroll é opt-in', () => {
  const onLoadOlder = vi.fn();
  const initial = props(groupEvents(), { onLoadOlder });
  const { container, root } = mount(createElement(MessageList, initial));
  expandGroup(container);
  expect(latestList().maintainScrollAtEnd).toBe(true);
  expect(latestList().nestedScrollEnabled).toBeUndefined();
  expect(latestList().onContentSizeChange).toBeUndefined();
  expect(latestList().initialScrollAtEnd).toBe(true);
  expect(latestList().maintainVisibleContentPosition).toBe(true);
  expect(latestList().recycleItems).toBe(false);
  expect(latestList().onStartReached).toBe(onLoadOlder);
  act(() => root.render(createElement(MessageList, { ...initial, nestedScrollEnabled: true })));
  expect(latestList().nestedScrollEnabled).toBe(true);
  act(() => root.render(createElement(MessageList, { ...initial, nestedScrollEnabled: false })));
  expect(latestList().nestedScrollEnabled).toBe(false);
});

it('alinhamento da conversa mantém fim por padrão e permite topo explícito', () => {
  const initial = props([event()]);
  const { root } = mount(createElement(MessageList, initial));
  expect(list.alignment.mock.lastCall?.[0]).toBe(true);
  act(() => root.render(createElement(MessageList, { ...initial, alignItemsAtEnd: false })));
  expect(list.alignment.mock.lastCall?.[0]).toBe(false);
});

it.each(['hook_prompt', 'hook_failed'])('o aviso %s conserva seu significado e mostra a causa', text => {
  const detail = 'hook-detail-127';
  const { container } = mount(createElement(MessageList, props([
    event({ id: 'hook-1', kind: 'notice', text, hook_error: detail }),
  ])));
  const row = container.querySelector('[data-row="hook-1"]');
  expect(row?.textContent).toBe(`${text === 'hook_failed' ? m.notice_hook_failed() : m.notice_hook_prompt()}\n${detail}`);
});

it('timeline orq entra antes do aviso genérico e atualiza só a fronteira do dia ao carregar anteriores', () => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(new Date(2026, 9, 16, 18));
  const orq: OrqEntry = { kind: 'advance', task: 3, line: { code: 'integrated', merge: true }, origin: 'orchestrator',
    sender: null, mark: null, alarm: false, rejected_round: null, body: '', question: null, parecer: null, error: null, decided_by: null };
  const notice = (id: string, day: number, hour: number) => event({ id, kind: 'notice', text: 'raw-orq', orq,
    ts: new Date(2026, 9, day, hour).getTime() / 1000 });
  const first = notice('first', 15, 9), second = notice('second', 15, 10), nextDay = notice('next-day', 16, 11);
  const initial = props([first, second, nextDay, event({ id: 'plain', kind: 'notice', text: 'interrupted' }),
    event({ id: 'unknown', kind: 'notice', text: 'future-notice' }), event({ id: 'user', text: 'user-content' })], { sessionName: 'sess', serverId: 'a' });
  const { container, root } = mount(createElement(MessageList, initial));
  const row = (id: string) => container.querySelector(`[data-row="${id}"]`)!;
  expect(row('first').textContent).toContain('fechada → merge → integração verde');
  expect(row('first').textContent).toContain('ontem · 09:00');
  expect(row('second').textContent).not.toContain('ontem');
  expect(row('second').textContent).toContain('10:00');
  expect(row('next-day').textContent).toContain('hoje · 11:00');
  expect(row('plain').textContent).toBe(m.notice_interrupted());
  expect(row('unknown').textContent).toBe('future-notice');
  expect(row('user').textContent).toBe('user-content');
  expect(container.textContent).not.toContain('raw-orq');
  const renderItem = list.render.mock.lastCall![0];
  const rows = list.data.mock.lastCall![0] as { id: string }[];
  act(() => root.render(createElement(MessageList, { ...initial, preview: 'streaming-preview' })));
  expect(list.render.mock.lastCall![0]).toBe(renderItem);
  expect(list.data.mock.lastCall![0]).toBe(rows);
  act(() => root.render(createElement(MessageList, { ...initial, events: [notice('earlier', 15, 8), ...initial.events] })));
  expect(row('earlier').textContent).toContain('ontem · 08:00');
  expect(row('first').textContent).not.toContain('ontem');
  const updated = list.data.mock.lastCall![0] as { id: string }[];
  expect(updated.find(item => item.id === 'first')).not.toBe(rows.find(item => item.id === 'first'));
  expect(updated.find(item => item.id === 'second')).toBe(rows.find(item => item.id === 'second'));
  expect(list.render.mock.lastCall![0]).toBe(renderItem);
  expect(fetch).not.toHaveBeenCalled();
});

describe('notas da entrega na conversa', () => {
  it.each([
    [{ desistiu: true }, undefined, ['native_delivery_failed', 'msg_nao_chegou_reenvie']],
    [{ id: 'held:one' }, undefined, ['native_delivery_failed']],
    [{ id: 'held:one', hook_error: 'hook recusou' }, undefined, ['hook recusou']],
    [{ id: 'real', hook_error: 'hook recusou' }, undefined, ['hook recusou']],
    [{ queued_delivered: true }, 'terminal_input_delivery_unproved', ['native_delivery_unproved']],
    [{ queued_delivered: false }, 'terminal_input_delivery_unproved', ['native_queue_held']],
    [{}, 'terminal_input_no_safe_delivery', ['native_no_safe_delivery']],
    [{ queued_delivered: true }, undefined, ['native_delivering']],
    [{ queued_delivered: false }, undefined, ['native_queued']],
    [{ queued_delivered: true, queued_consumed: true }, undefined, []],
  ] as [Partial<ChatEvent>, string | undefined, string[]][])('evento %j, problema %s', (patch, problem, expected) => {
    const { container } = mount(createElement(MessageList, props([event(patch)], {
      stateEvent: { session: 'sess', state: 'working', problema: problem },
    })));
    expect(container.textContent).toBe('mensagem' + expected.join(' · '));
    expect(discardButtons(container)).toHaveLength(0);
  });

  it.each([
    [{ queued_delivered: true }, 'terminal_input_delivery_unproved'],
    [{ queued_delivered: false }, 'terminal_input_delivery_unproved'],
    [{ queued_delivered: false }, 'terminal_input_no_safe_delivery'],
    [{ desistiu: true }, undefined],
    [{ hook_error: 'hook recusou' }, undefined],
    [{ queued_delivered: true }, undefined],
    [{ queued_delivered: false }, undefined],
    [{ id: 'held:one', hook_error: 'hook recusou' }, undefined],
  ] as [Partial<ChatEvent>, string | undefined][])('consumida fica sólida, sem nota ou descarte: %j/%s', (patch, problem) => {
    const { container } = mount(createElement(MessageList, props([event({ ...patch, queued_consumed: true })], {
      sessionName: 'sess', serverId: 'a', stateEvent: { session: 'sess', state: 'working', problema: problem },
    })));
    expect(container.querySelector('[data-user]')?.textContent).toBe('mensagem');
    expect(container.querySelector('[data-user]')?.closest('[style*="opacity"]')).toBeNull();
    expect(container.textContent).toBe('mensagem');
    expect(discardButtons(container)).toHaveLength(0);
    expect(fetch).not.toHaveBeenCalled();
  });

  it('desistida fica sólida e não aparece como esperando, mesmo trabalhando', () => {
    const { container } = mount(createElement(MessageList, props([event({ desistiu: true, queued_delivered: false })])));
    expect(container.querySelector('[data-user]')?.closest('[style*="opacity"]')).toBeNull();
    expect(container.textContent).toContain(m.msg_nao_chegou_reenvie());
    expect(container.textContent).not.toContain(m.native_queued());
  });

  it('mudança do problema refaz a nota; a entrada do painel não duplica na conversa', () => {
    const waiting = event({ queued_delivered: false });
    const { container, root } = mount(createElement(MessageList, props([waiting])));
    expect(container.textContent).toContain(m.native_queued());
    act(() => root.render(createElement(MessageList, props([waiting], {
      stateEvent: { session: 'sess', state: 'working', problema: 'terminal_input_delivery_unproved' },
    }))));
    expect(container.textContent).toContain(m.native_queue_held());
    act(() => root.render(createElement(MessageList, props([waiting], {
      queuePanel: { entries: [{ id: 'one', text: 'mensagem', ts: null }], actions: true },
    }))));
    expect(container.querySelector('[data-user]')).toBeNull();
    expect(container.textContent).toBe('');
  });

  it.each([
    [{ desistiu: true }, undefined, 1],
    [{ queued_delivered: true }, 'terminal_input_delivery_unproved', 1],
    [{ queued_delivered: false }, 'terminal_input_delivery_unproved', 0],
    [{ queued_delivered: false }, 'terminal_input_no_safe_delivery', 0],
    [{ queued_delivered: true }, undefined, 0],
    [{ queued_delivered: false }, undefined, 0],
    [{ id: 'held:one', desistiu: true }, undefined, 0],
    [{ id: 'real', hook_error: 'hook recusou' }, undefined, 0],
  ] as [Partial<ChatEvent>, string | undefined, number][])('Descartar só na bolha permitida %j/%s', (patch, problem, count) => {
    const { container } = mount(createElement(MessageList, props([event(patch)], {
      sessionName: 'sess', serverId: 'a', stateEvent: { session: 'sess', state: 'working', problema: problem },
    })));
    expect(discardButtons(container)).toHaveLength(count);
    expect(fetch).not.toHaveBeenCalled();
  });

  it.each([{ sessionName: 'sess' }, { serverId: 'a' }, {}])('leitura sem identidade completa não oferece descarte: %j', (identity) => {
    const { container } = mount(createElement(MessageList, props([event({ desistiu: true })], identity)));
    expect(container.textContent).toContain(m.native_delivery_failed());
    expect(discardButtons(container)).toHaveLength(0);
    expect(fetch).not.toHaveBeenCalled();
  });
});

it('renderItem mantém identidade no tique da sessão e reage a provider/headless', () => {
  const initial = props([event({ queued_delivered: true })]);
  const { container, root } = mount(createElement(MessageList, initial));
  const renderItem = list.render.mock.lastCall![0];
  expect(container.querySelector('[data-user]')?.closest('[style*="opacity"]')).not.toBeNull();

  const tickSession = { ...session, last_activity: 2, status_line: 'tick', git_added: 1 };
  act(() => root.render(createElement(MessageList, { ...initial, session: tickSession })));
  expect(list.render.mock.lastCall![0]).toBe(renderItem);

  act(() => root.render(createElement(MessageList, { ...initial, session: { ...tickSession, headless: true } })));
  const headlessRender = list.render.mock.lastCall![0];
  expect(headlessRender).not.toBe(renderItem);
  expect(container.querySelector('[data-user]')?.closest('[style*="opacity"]')).toBeNull();

  act(() => root.render(createElement(MessageList, initial)));
  const terminalRender = list.render.mock.lastCall![0];
  expect(terminalRender).not.toBe(headlessRender);
  expect(container.querySelector('[data-user]')?.closest('[style*="opacity"]')).not.toBeNull();

  act(() => root.render(createElement(MessageList, { ...initial, session: { ...session, provider: 'codex' } })));
  expect(list.render.mock.lastCall![0]).not.toBe(terminalRender);
  expect(container.querySelector('[data-user]')?.closest('[style*="opacity"]')).toBeNull();
});

it('botões de descarte identificam a mensagem com trecho normalizado e limitado', () => {
  const text = `  first-message\n\t${'x'.repeat(90)}`;
  const { container } = mount(createElement(MessageList, props([
    event({ text, desistiu: true }), event({ id: 'queued-two', text: 'second-message', desistiu: true }),
  ], { sessionName: 'sess', serverId: 'a' })));
  const buttons = discardButtons(container);
  expect(buttons).toHaveLength(2);
  expect(buttons.map((button) => button.getAttribute('aria-label'))).toEqual([
    m.native_queue_discard_message({ text: `first-message ${'x'.repeat(66)}` }),
    m.native_queue_discard_message({ text: 'second-message' }),
  ]);
  expect(buttons.map((button) => button.textContent)).toEqual([m.native_queue_discard(), m.native_queue_discard()]);
  expect(fetch).not.toHaveBeenCalled();
});

it('ação pública mantém erro e bolha; ocupado desabilita, e ACK tira só o alvo no servidor da rota', async () => {
  const chat = chatStore('a', 'sess');
  chat.use.setState({ events: [event({ id: 'queued-raw/id', desistiu: true }), event({ id: 'queued-other', text: 'vizinha', desistiu: true })] });
  function Conversation() {
    return createElement(MessageList, props(chat.use((s) => s.events), { sessionName: 'sess', serverId: 'a' }));
  }
  const { container } = mount(createElement(Conversation));
  let resolve!: (response: Response) => void;
  const fetchMock = vi.fn((_url: string, _init?: RequestInit) => new Promise<Response>((done) => { resolve = done; }));
  vi.stubGlobal('fetch', fetchMock);
  act(() => discardButtons(container)[0].click());
  expect(discardButtons(container).every((button) => button.disabled)).toBe(true);
  expect(container.textContent).toContain('loading');
  expect(container.textContent).toContain('mensagem');
  expect(fetchMock.mock.calls[0][0]).toBe('http://10.0.0.1:8765/api/sessions/sess/queue/raw%2Fid');
  expect(fetchMock.mock.calls[0][1]?.method).toBe('DELETE');
  expect(new Headers(fetchMock.mock.calls[0][1]?.headers).get('Authorization')).toBe('Bearer token-a');
  await act(async () => { resolve(new Response('{"detail":"descarte recusado"}', { status: 409 })); });
  expect(container.querySelector('[role="alert"]')?.textContent).toBe('descarte recusado');
  expect(container.textContent).toContain('mensagem');
  expect(discardButtons(container).every((button) => !button.disabled)).toBe(true);
  act(() => discardButtons(container)[0].click());
  await act(async () => { resolve(new Response('{"ok":true}', { status: 200 })); });
  expect(container.textContent).not.toContain('mensagem');
  expect(container.textContent).toContain('vizinha');
  expect(container.querySelector('[role="alert"]')).toBeNull();
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['queued-other']);
});

it('descarte mostra só o erro próprio, expira em 6s sem apagar bolha e a próxima ação não herda a causa', async () => {
  vi.useFakeTimers();
  const chat = chatStore('a', 'sess');
  chat.use.setState({ events: [event({ text: 'first-message', desistiu: true }), event({ id: 'queued-two', text: 'second-message', desistiu: true })] });
  function Conversation() {
    return createElement(MessageList, props(chat.use((s) => s.events), { sessionName: 'sess', serverId: 'a' }));
  }
  const { container } = mount(createElement(Conversation));
  let resolve!: (response: Response) => void;
  vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>((done) => { resolve = done; })));
  const unsubscribe = chat.use.subscribe((state, previous) => {
    if (previous.queueBusy && !state.queueBusy) chat.use.setState({ queueError: 'other-action' });
  });
  try {
    act(() => discardButtons(container)[0].click());
    await act(async () => { resolve(new Response('{"detail":"first-error"}', { status: 409 })); });
    expect(chat.use.getState().queueError).toBe('other-action');
    expect([...container.querySelectorAll('[role="alert"]')].map((alert) => alert.textContent)).toEqual(['first-error']);
  } finally {
    unsubscribe();
  }
  await act(async () => { await vi.advanceTimersByTimeAsync(5999); });
  expect(container.querySelector('[role="alert"]')?.textContent).toBe('first-error');
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  expect(container.querySelector('[role="alert"]')).toBeNull();
  expect([...container.querySelectorAll('[data-user]')].map((bubble) => bubble.textContent)).toEqual(['first-message', 'second-message']);
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['queued-one', 'queued-two']);

  act(() => discardButtons(container)[1].click());
  expect(container.querySelector('[role="alert"]')).toBeNull();
  await act(async () => { resolve(new Response('{"detail":"second-error"}', { status: 409 })); });
  expect([...container.querySelectorAll('[role="alert"]')].map((alert) => alert.textContent)).toEqual(['second-error']);
  act(() => discardButtons(container)[1].click());
  expect(container.querySelector('[role="alert"]')).toBeNull();
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['queued-one', 'queued-two']);
  await act(async () => { resolve(new Response('{"ok":true}', { status: 200 })); });
  expect(container.querySelector('[role="alert"]')).toBeNull();
  expect([...container.querySelectorAll('[data-user]')].map((bubble) => bubble.textContent)).toEqual(['first-message']);
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['queued-one']);
});

it('servidor removido aparece na própria bolha e desmontar limpa o timer local', async () => {
  vi.useFakeTimers();
  const { container, root } = mount(createElement(MessageList, props([event({ desistiu: true })], {
    sessionName: 'sess', serverId: 'missing',
  })));
  await act(async () => { discardButtons(container)[0].click(); });
  expect(fetch).not.toHaveBeenCalled();
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.chat_servidor_removido());
  expect(container.querySelector('[data-user]')?.textContent).toBe('mensagem');
  expect(discardButtons(container)[0].disabled).toBe(false);
  await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
  await act(async () => { discardButtons(container)[0].click(); });
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.chat_servidor_removido());
  // O aviso global continua no store; só o timer da bolha é cancelado na desmontagem.
  expect(vi.getTimerCount()).toBe(2);
  act(() => root.render(null));
  expect(vi.getTimerCount()).toBe(1);
});
