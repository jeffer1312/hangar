// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, type Server } from '@hangar/core';
import { setQueueDeliveryForeground, useDeliveryMode, useQueueDelivery } from './useQueueDelivery';
import { DeliveryModeRow } from '../features/config/DeliveryModeRow';
import * as m from '../paraglide/messages';

const disk = vi.hoisted(() => new Map<string, string>());
const diskFull = vi.hoisted(() => ({ on: false }));
vi.mock('../stores/prefs', () => ({ prefs: { getString: (key: string) => disk.get(key), set: (key: string, value: string) => {
  if (diskFull.on) throw new Error('disk full');
  disk.set(key, value);
} } }));
// Cada opção do menu nativo vira um botão: o teste escolhe como a pessoa escolheria no menu.
vi.mock('@react-native-menu/menu', async () => {
  const { createElement: h } = await import('react');
  type Action = { id: string; title: string; subtitle?: string; state?: string; attributes?: { disabled?: boolean } };
  return { MenuView: ({ actions, onPressAction, children }: { actions: Action[]; onPressAction: (e: { nativeEvent: { event: string } }) => void; children: unknown }) =>
    h('div', null, ...actions.map((a) => h('button', { key: a.id, 'aria-label': a.title, 'data-subtitle': a.subtitle, 'aria-pressed': a.state === 'on',
      disabled: !!a.attributes?.disabled, onClick: () => onPressAction({ nativeEvent: { event: a.id } }) })), children) };
});
vi.mock('expo-router', () => ({ useFocusEffect: () => {} }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {} }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../ui/haptics', () => ({ haptic: { selection: () => {}, success: () => {}, error: () => {} } }));
const active = vi.hoisted(() => ({ server: null as Server | null }));
vi.mock('../stores/servers', () => ({ useServers: (selector: (s: { active: () => Server | null }) => unknown) => selector({ active: () => active.server }) }));
const toastErro = vi.hoisted(() => vi.fn());
vi.mock('../ui/Toast', () => ({ toast: { erro: toastErro } }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const first: Server = { id: 'one', label: 'one', baseUrl: 'https://one.test', token: 'one-token' };
const second: Server = { id: 'two', label: 'two', baseUrl: 'https://two.test', token: 'two-token' };
// A linha das Configurações > Geral, no servidor ativo dado.
function Controls({ server = first }: { server?: Server }) {
  active.server = server;
  return createElement(DeliveryModeRow);
}
let root: Root;
let container: HTMLDivElement;
const button = (name: string) => container.querySelector<HTMLButtonElement>(`button[aria-label="${name}"]`)!;
beforeEach(() => {
  useDeliveryMode.setState({ mode: 'queued' }); disk.clear(); diskFull.on = false; toastErro.mockClear(); active.server = null;
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', onUnauthorized: vi.fn(), origin: null, createEventSource: vi.fn() });
  vi.stubGlobal('fetch', vi.fn());
  container = document.createElement('div'); root = createRoot(container);
});
afterEach(() => { act(() => root.unmount()); vi.useRealTimers(); vi.unstubAllGlobals(); });

it('troca Fila/Direto/Jev/Na conversa na linha das Configurações e grava uma única escolha', async () => {
  vi.mocked(fetch).mockResolvedValue(new Response('{"available":true}'));
  await act(async () => root.render(createElement(Controls)));
  expect(container.textContent).toContain(m.delivery_mode_title());
  expect(container.textContent).toContain(m.delivery_mode_queue_desc());
  act(() => button(m.delivery_mode_direct()).click());
  expect(useDeliveryMode.getState().mode).toBe('direct');
  act(() => button(m.delivery_mode_jev()).click());
  expect(useDeliveryMode.getState().mode).toBe('jev');
  expect(disk.get('queue.deliveryMode')).toBe('jev');
  act(() => button(m.delivery_mode_conversation()).click());
  expect(useDeliveryMode.getState().mode).toBe('conversation');
  expect(disk.get('queue.deliveryMode')).toBe('conversation');
  // A descrição acompanha a escolha; o menu traz a de cada opção.
  expect(container.textContent).toContain(m.mobile_delivery_mode_conversation_desc());
  expect(button(m.delivery_mode_direct()).getAttribute('data-subtitle')).toBe(m.delivery_mode_direct_desc());
  expect(button(m.delivery_mode_conversation()).getAttribute('aria-pressed')).toBe('true');
  act(() => button(m.delivery_mode_queue()).click());
  expect(useDeliveryMode.getState().mode).toBe('queued');
});

it('mudança de servidor invalida disponibilidade e ignora a resposta antiga', async () => {
  let resolveFirst!: (response: Response) => void;
  let resolveSecond!: (response: Response) => void;
  vi.mocked(fetch).mockImplementation((url) => new Promise<Response>((resolve) => {
    if (String(url).startsWith('https://one.test')) resolveFirst = resolve;
    else resolveSecond = resolve;
  }));
  useDeliveryMode.setState({ mode: 'jev' });
  await act(async () => root.render(createElement(Controls)));
  expect(button(m.delivery_mode_jev()).disabled).toBe(true);
  expect(container.textContent).toContain(m.native_queue_jev_loading().replace(/…$/, ''));
  await act(async () => root.render(createElement(Controls, { server: second })));
  await act(async () => resolveSecond(new Response('{"available":false}')));
  expect(container.textContent).toContain(m.native_queue_jev_setup());
  await act(async () => resolveFirst(new Response('{"available":true}')));
  // A resposta velha (disponível) não ressuscita o Jev, que sem configuração nem aparece.
  expect(button(m.delivery_mode_jev())).toBeNull();
  expect(container.textContent).toContain(m.native_queue_jev_setup());
});

it.each([404, 503])('explica falha %s e deixa Direto disponível', async (status) => {
  vi.mocked(fetch).mockResolvedValue(new Response('{"detail":"fixture-error"}', { status }));
  useDeliveryMode.setState({ mode: 'jev' });
  await act(async () => root.render(createElement(Controls)));
  expect(button(m.delivery_mode_jev()).disabled).toBe(true);
  expect(button(m.delivery_mode_direct()).disabled).toBe(false);
  expect(container.textContent).toContain(status === 404 ? m.native_queue_jev_unsupported() : 'fixture-error');
});

it('reconfere configuração depois de trinta segundos e limpa o timer ao sair', async () => {
  vi.useFakeTimers();
  vi.mocked(fetch).mockImplementation(async () => new Response('{"available":true}'));
  await act(async () => root.render(createElement(Controls)));
  expect(fetch).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(29_999));
  expect(fetch).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(fetch).toHaveBeenCalledTimes(2);
  act(() => root.render(null));
  await act(async () => vi.advanceTimersByTimeAsync(30_000));
  expect(fetch).toHaveBeenCalledTimes(2);
});

function Status({ server = first }: { server?: Server }) {
  return createElement('span', { 'data-status': useQueueDelivery(server, true).status });
}
const statuses = () => [...container.querySelectorAll('[data-status]')].map((el) => el.getAttribute('data-status'));

it('composer e pergunta da mesma conversa dividem uma leitura só', async () => {
  vi.mocked(fetch).mockImplementation(async () => new Response('{"available":true}'));
  await act(async () => root.render(createElement('div', null, createElement(Status), createElement(Status))));
  expect(statuses()).toEqual(['ready', 'ready']);
  expect(fetch).toHaveBeenCalledTimes(1);
});

it('para de conferir com o app em segundo plano e confere na volta', async () => {
  vi.useFakeTimers();
  vi.mocked(fetch).mockImplementation(async () => new Response('{"available":true}'));
  await act(async () => root.render(createElement(Status)));
  expect(fetch).toHaveBeenCalledTimes(1);
  act(() => setQueueDeliveryForeground(false));
  await act(async () => vi.advanceTimersByTimeAsync(90_000));
  expect(fetch).toHaveBeenCalledTimes(1);
  await act(async () => setQueueDeliveryForeground(true));
  expect(fetch).toHaveBeenCalledTimes(2);
});

it('uma falha passageira não desliga o Jev; a segunda seguida, sim', async () => {
  vi.useFakeTimers();
  vi.mocked(fetch).mockImplementationOnce(async () => new Response('{"available":true}'))
    .mockImplementation(async () => new Response('{"detail":"fixture-error"}', { status: 503 }));
  await act(async () => root.render(createElement(Status)));
  expect(statuses()).toEqual(['ready']);
  await act(async () => vi.advanceTimersByTimeAsync(30_000));
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(statuses()).toEqual(['ready']);
  await act(async () => vi.advanceTimersByTimeAsync(30_000));
  expect(statuses()).toEqual(['error']);
});

it('modo que não grava no aparelho vale na hora e avisa', () => {
  diskFull.on = true;
  act(() => useDeliveryMode.getState().setMode('direct'));
  expect(useDeliveryMode.getState().mode).toBe('direct');
  expect(toastErro).toHaveBeenCalledWith(m.mobile_queue_mode_save_failed());
  diskFull.on = false;
  act(() => useDeliveryMode.getState().setMode('queued'));
  expect(useDeliveryMode.getState().mode).toBe('queued');
  expect(toastErro).toHaveBeenCalledTimes(1);
});

it('404 depois de pronto vira sem suporte na hora; a falha de antes da pausa não conta na volta', async () => {
  vi.useFakeTimers();
  vi.mocked(fetch).mockImplementationOnce(async () => new Response('{"available":true}'))
    .mockImplementationOnce(async () => new Response('{}', { status: 503 }))
    .mockImplementationOnce(async () => new Response('{}', { status: 503 }))
    .mockImplementation(async () => new Response('{}', { status: 404 }));
  await act(async () => root.render(createElement(Status)));
  await act(async () => vi.advanceTimersByTimeAsync(30_000));
  expect(statuses()).toEqual(['ready']);
  act(() => setQueueDeliveryForeground(false));
  await act(async () => setQueueDeliveryForeground(true));
  expect(fetch).toHaveBeenCalledTimes(3);
  expect(statuses()).toEqual(['ready']);
  await act(async () => vi.advanceTimersByTimeAsync(5_000));
  expect(statuses()).toEqual(['unsupported']);
});
