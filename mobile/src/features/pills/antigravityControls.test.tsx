// @vitest-environment happy-dom
import { act, createElement, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useModelControl } from './modelControl';
import { usePermissionControl } from './permissionControl';
import type { Chosen } from './pills';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { chatStore, _resetChatsForTests } from '../../stores/chat';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const api = vi.hoisted(() => ({ models: vi.fn(), setModel: vi.fn(), modes: vi.fn(), setMode: vi.fn() }));
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  getAntigravityModels: api.models, setAntigravityModel: api.setModel,
  getPermissionModes: api.modes, setPermissionMode: api.setMode,
}));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {} }) }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {} }));

const server = { id: 'own', label: 'Own', baseUrl: 'http://owner.test', token: 'test' };
const roots: Root[] = [];
const catalog = { current: 'gemini-4-neon-low', models: [
  { id: 'gemini-4-neon-low', name: 'Gemini 4 Neon (Low)' },
  { id: 'gemini-4-neon-medium', name: 'Gemini 4 Neon (Medium)' },
] };

function ModelHarness() {
  const [chosen, setChosen] = useState<Chosen>({});
  const model = useModelControl({ serverId: server.id, name: 'agy', provider: 'antigravity', chosen, onChosen: setChosen, close: () => {} });
  return createElement('div', null,
    createElement('output', null, model.display),
    createElement('button', { onClick: () => void model.load() }, 'load'),
    model.notice ? createElement('p', { role: 'alert' }, model.notice) : null,
    ...model.items.map(item => createElement('button', { key: item.id, 'aria-pressed': !!item.selected, onClick: () => void model.select(item) }, item.label)));
}

function ModeHarness() {
  const mode = usePermissionControl({ serverId: server.id, name: 'agy', provider: 'antigravity' });
  return createElement('div', null,
    createElement('output', null, `${mode.enabled}:${mode.current}:${mode.canProbe}`),
    ...mode.options.map(option => createElement('button', { key: option, onClick: () => void mode.select(option) }, option)));
}

async function mount(component: () => ReturnType<typeof createElement>) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(createElement(component)));
  return container;
}
const buttons = (container: HTMLElement) => [...container.querySelectorAll<HTMLButtonElement>('button')];
const click = (container: HTMLElement, label: string) => act(async () => buttons(container).find(b => b.textContent === label)!.click());

beforeEach(() => {
  vi.resetAllMocks();
  useServers.setState({ servers: [server], activeId: server.id, ready: true, vaultReadable: true, aviso: null });
  useSessions.setState({ rows: [{ name: 'agy', state: 'idle', provider: 'antigravity', model: 'gemini-4-neon-low',
    serverId: server.id, serverLabel: server.label, serverColor: '#00f' }] });
  vi.stubGlobal('fetch', vi.fn(() => { throw new Error('unexpected-network'); }));
});
afterEach(async () => {
  await act(async () => roots.splice(0).forEach(root => root.unmount()));
  useSessions.setState({ rows: [] });
  _resetChatsForTests();
  vi.unstubAllGlobals();
});

it('Antigravity: a pílula de modelo lista o agy models, marca o atual e troca pelo id', async () => {
  api.models.mockResolvedValue(catalog);
  api.setModel.mockResolvedValue({ ok: true, current: 'gemini-4-neon-medium' });
  const container = await mount(ModelHarness);
  // Antes do catálogo, o nome sai do id da lista, como o agy escreve.
  expect(container.querySelector('output')!.textContent).toBe('Gemini 4 Neon (Low)');
  await click(container, 'load');
  expect(api.models).toHaveBeenCalledWith('agy', server);
  expect(buttons(container).filter(b => b.getAttribute('aria-pressed') === 'true').map(b => b.textContent)).toEqual(['Gemini 4 Neon (Low)']);
  await click(container, 'Gemini 4 Neon (Medium)');
  expect(api.setModel).toHaveBeenCalledExactlyOnceWith('agy', 'gemini-4-neon-medium', server);
  expect(container.querySelector('output')!.textContent).toBe('Gemini 4 Neon (Medium)');
  expect(buttons(container).filter(b => b.getAttribute('aria-pressed') === 'true').map(b => b.textContent)).toEqual(['Gemini 4 Neon (Medium)']);
});

it('Antigravity: troca recusada mostra o erro e não marca o modelo pedido', async () => {
  api.models.mockResolvedValue(catalog);
  api.setModel.mockRejectedValue(Object.assign(new Error('agy-model-refused'), { status: 422, code: 'erro_modelo_antigravity' }));
  const container = await mount(ModelHarness);
  await click(container, 'load');
  await click(container, 'Gemini 4 Neon (Medium)');
  expect(container.querySelector('[role="alert"]')!.textContent).toBe('agy-model-refused');
  expect(container.querySelector('output')!.textContent).toBe('Gemini 4 Neon (Low)');
});

it('Antigravity: o modo segue o campo do Claude no SSE e troca pela rota do Claude, sem sonda', async () => {
  // O GET relê o rodapé do agy a cada mudança de estado: devolve o que o terminal mostra agora.
  let footer = 'manual';
  api.modes.mockImplementation(async () => ({ current: footer, modes: [], sondavel: false, previous_non_plan: 'manual' }));
  api.setMode.mockImplementation(async (_: string, mode: string) => { footer = mode; return { mode, current: mode, previous_non_plan: 'manual' }; });
  const container = await mount(ModeHarness);
  await act(async () => Promise.resolve());
  expect(api.modes).toHaveBeenCalledWith('agy', false, server);
  expect(container.querySelector('output')!.textContent).toBe('true:manual:false');
  // O ciclo do agy, sem o bypass que só existe na abertura.
  expect(buttons(container).map(b => b.textContent)).toEqual(['manual', 'auto', 'acceptEdits', 'plan']);
  footer = 'auto';
  await act(async () => chatStore(server.id, 'agy').use.setState({ stateEvent: { session: 'agy', state: 'idle', claude_permission_mode: 'auto' } as never }));
  expect(container.querySelector('output')!.textContent).toContain(':auto:');
  await click(container, 'plan');
  expect(api.setMode).toHaveBeenCalledExactlyOnceWith('agy', 'plan', server);
  expect(container.querySelector('output')!.textContent).toContain(':plan:');
});

it('Antigravity aberto em bypass mostra o modo atual junto do ciclo', async () => {
  api.modes.mockResolvedValue({ current: 'bypassPermissions', modes: [], sondavel: false, previous_non_plan: 'bypassPermissions' });
  const container = await mount(ModeHarness);
  await act(async () => Promise.resolve());
  expect(buttons(container).map(b => b.textContent)).toEqual(['manual', 'auto', 'acceptEdits', 'plan', 'bypassPermissions']);
});
