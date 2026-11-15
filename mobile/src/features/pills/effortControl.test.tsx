// @vitest-environment happy-dom
import { act, createElement, useEffect, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { CodexModelsResponse } from '@hangar/core';
import { useEffortControl } from './effortControl';
import type { Chosen } from './pills';
import { useServers } from '../../stores/servers';
import { _resetChatsForTests } from '../../stores/chat';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const api = vi.hoisted(() => ({ read: vi.fn(), write: vi.fn() }));
vi.mock('@hangar/core', async original => ({ ...await original<typeof import('@hangar/core')>(), getCodexModels: api.read, setCodexModel: api.write }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {} }) }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {} }));

const server = { id: 'own', label: 'Own', baseUrl: 'http://owner.test', token: 'test' };
const roots: Root[] = [];
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(yes => { resolve = yes; });
  return { promise, resolve };
}
const catalog = (model: string, levels: [string, ...string[]]): CodexModelsResponse => ({
  current: { model, effort: levels[0] }, models: [{ model, displayName: model, description: null, defaultEffort: levels[0], efforts: levels.map(value => ({ value, description: null })) }],
});

function Harness() {
  const [chosen, setChosen] = useState<Chosen>({ model: 'model-a', effort: 'high' });
  const effort = useEffortControl({ serverId: server.id, name: 'sess', provider: 'codex', model: chosen.model ?? null, chosen, onChosen: setChosen, close: () => {} });
  useEffect(() => { void effort.load(); }, [chosen.model]);
  return createElement('div', null,
    createElement('button', { onClick: () => setChosen({ model: 'model-b', effort: 'medium' }) }, 'model-b'),
    effort.loading ? createElement('span', { role: 'progressbar' }) : null,
    ...effort.items.map(item => createElement('button', { key: item.label, onClick: () => void effort.select(item.label) }, item.label)));
}

beforeEach(() => {
  vi.resetAllMocks();
  useServers.setState({ servers: [server], activeId: server.id, ready: true, vaultReadable: true, aviso: null });
  vi.stubGlobal('fetch', vi.fn(() => { throw new Error('unexpected-network'); }));
  api.write.mockResolvedValue({ ok: true });
});
afterEach(async () => {
  await act(async () => roots.splice(0).forEach(root => root.unmount()));
  _resetChatsForTests();
  vi.unstubAllGlobals();
});

it.each([false, true])('resposta antiga não troca os níveis nem o modelo novo, antiga primeiro=%s', async oldFirst => {
  const old = deferred<CodexModelsResponse>();
  const latest = deferred<CodexModelsResponse>();
  let active = 'model-a';
  api.read.mockImplementation(() => active === 'model-a' ? old.promise : latest.promise);
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  try {
    await act(async () => root.render(createElement(Harness)));
    active = 'model-b';
    await act(async () => container.querySelector<HTMLButtonElement>('button')!.click());
    if (oldFirst) {
      await act(async () => old.resolve(catalog('model-a', ['high', 'low'])));
      expect(container.querySelector('[role="progressbar"]')).not.toBeNull();
      await act(async () => latest.resolve(catalog('model-b', ['medium', 'max'])));
    } else {
      await act(async () => latest.resolve(catalog('model-b', ['medium', 'max'])));
      await act(async () => old.resolve(catalog('model-a', ['high', 'low'])));
    }
    expect([...container.querySelectorAll('button')].map(button => button.textContent)).toEqual(['model-b', 'medium', 'max']);
    await act(async () => [...container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'max')!.click());
    expect(api.write).toHaveBeenCalledExactlyOnceWith('sess', 'model-b', 'max', server);
  } finally {
    await act(async () => { old.resolve(catalog('model-a', ['high', 'low'])); latest.resolve(catalog('model-b', ['medium', 'max'])); });
  }
});
