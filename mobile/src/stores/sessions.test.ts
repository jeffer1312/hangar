import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { configureApi } from '@hangar/core';
import { useServers } from './servers';
import { useSessions, _resetSessionsForTests } from './sessions';

// mock SecureStore para useServers não quebrar se load for chamado
vi.mock('expo-secure-store', () => {
  const m = new Map<string, string>();
  return {
    getItemAsync: async (k: string) => m.get(k) ?? null,
    setItemAsync: async (k: string, v: string) => { m.set(k, v); },
    deleteItemAsync: async (k: string) => { m.delete(k); },
  };
});

// fábrica de EventSource falso injetada via configureApi
type FakeES = {
  url: string;
  opts: Record<string, unknown>;
  listeners: Record<string, ((e: { data: string }) => void)[]>;
  close: ReturnType<typeof vi.fn>;
  trigger: (type: string, data: string) => void;
  onerror: ((e: unknown) => void) | null;
};

let created: FakeES[] = [];

function fakeCreateEventSource(url: string, opts: { withCredentials: boolean; headers?: Record<string, string> }): FakeES & { addEventListener: any; removeEventListener: any; close: any; onerror: any; onopen: any; readyState: number } {
  const fake: FakeES = {
    url,
    opts,
    listeners: {},
    close: vi.fn(),
    onerror: null,
    trigger(type, data) {
      (this.listeners[type] ?? []).forEach((fn) => fn({ data }));
    },
  };
  const wrapper = {
    get url() { return fake.url; },
    addEventListener(type: string, fn: (e: { data: string }) => void) {
      (fake.listeners[type] ??= []).push(fn);
    },
    removeEventListener() {},
    close: fake.close,
    get onerror() { return fake.onerror; },
    set onerror(fn: ((e: unknown) => void) | null) { fake.onerror = fn; },
    get onopen() { return null; },
    set onopen(_: unknown) {},
    get readyState() { return 1; },
    trigger: fake.trigger.bind(fake),
    listeners: fake.listeners,
    opts: fake.opts,
  } as unknown as FakeES & { addEventListener: any; removeEventListener: any; close: any; onerror: any; onopen: any; readyState: number };
  created.push(wrapper as unknown as FakeES);
  return wrapper as any;
}

beforeEach(() => {
  created = [];
  _resetSessionsForTests();
  useServers.setState({ servers: [], activeId: null, ready: true });
  configureApi({
    getBaseUrl: () => useServers.getState().active()?.baseUrl ?? '',
    getToken: () => useServers.getState().active()?.token ?? null,
    onUnauthorized: () => {},
    origin: null,
    createEventSource: fakeCreateEventSource as any,
  });
  // um servidor ativo
  const s = { id: 'srv1', label: 'srv1', baseUrl: 'http://10.0.0.1:8765', token: 'tok' };
  useServers.setState({ servers: [s], activeId: s.id, ready: true });
});

afterEach(() => {
  _resetSessionsForTests();
});

test('dois retain() = um createEventSource; release dos dois = close() uma vez', () => {
  const rel1 = useSessions.getState().retain();
  expect(created.length).toBe(1);
  const rel2 = useSessions.getState().retain();
  expect(created.length).toBe(1); // segundo retain não abre novo stream
  // ainda aberto
  expect(created[0].close).not.toHaveBeenCalled();
  rel1();
  expect(created[0].close).not.toHaveBeenCalled(); // ainda tem 1 consumidor
  rel2();
  expect(created[0].close).toHaveBeenCalledTimes(1);
});

test('um message com 2 sessões → order() com 2 slots ordenados como o front (aggregateSessions + sortSessions)', () => {
  useSessions.getState().retain();
  const es = created[0] as unknown as FakeES & { trigger: (t: string, d: string) => void };
  const payload = JSON.stringify([
    { name: 'beta', state: 'idle', jsonl: '/tmp/beta.jsonl' },
    { name: 'alpha', state: 'awaiting_input', jsonl: '/tmp/alpha.jsonl' },
  ]);
  es.trigger('sessions', payload);
  const ordered = useSessions.getState().order();
  expect(ordered).toHaveLength(2);
  // sortSessions: awaiting_input primeiro, depois alfabético
  expect(ordered.map((s) => s.name)).toEqual(['alpha', 'beta']);
  // rows também deve ter 2
  expect(useSessions.getState().rows).toHaveLength(2);
});

test('mensagem malformada mantém lista anterior e marca offline sem quebrar', () => {
  useSessions.getState().retain();
  const es = created[0] as unknown as FakeES & { trigger: (t: string, d: string) => void };
  const good = JSON.stringify([{ name: 'x', state: 'idle', jsonl: '/j/x.jsonl' }]);
  es.trigger('sessions', good);
  expect(useSessions.getState().rows).toHaveLength(1);
  es.trigger('sessions', 'not-json');
  // mantém a anterior (stale) mas error vira offline
  expect(useSessions.getState().rows).toHaveLength(1);
  expect(useSessions.getState().byServer[0].error).toBeTruthy();
});
