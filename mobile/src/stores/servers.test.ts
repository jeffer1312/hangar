import { describe, test, expect, vi, beforeEach } from 'vitest';

vi.mock('expo-secure-store', () => {
  const m = new Map<string, string>();
  return {
    getItemAsync: async (k: string) => m.get(k) ?? null,
    setItemAsync: async (k: string, v: string) => {
      m.set(k, v);
    },
    deleteItemAsync: async (k: string) => {
      m.delete(k);
    },
  };
});

import { useServers } from './servers';

beforeEach(async () => {
  // reset store
  useServers.setState({ servers: [], activeId: null, ready: false });
  const SecureStore = await import('expo-secure-store');
  // clear mock map by deleting known key
  await SecureStore.deleteItemAsync('cp_servers_v1');
});

test('add/setActive/remove persistem', async () => {
  await useServers.getState().load();
  const s = useServers.getState().add({ baseUrl: 'http://10.0.0.2:8765/', token: 'abc' });
  expect(s.baseUrl).toBe('http://10.0.0.2:8765'); // sem barra final
  expect(useServers.getState().active()?.id).toBe(s.id);
  useServers.getState().remove(s.id);
  expect(useServers.getState().active()).toBeNull();
});
