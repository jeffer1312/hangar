import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getTagsForServer, putTagsForServer } from './api';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import { _limparEsfriamentoParaTestes } from './esfriamento';
import { aggregateSessions, type Slot } from './sessions';
import { serverColor, type Server } from './servers';
import { machineLabel, parseTagsEvent, TAG_COLOR_HEX } from './tags';
import type { SessionInfo } from './types';

const server = (id: string, extra: Partial<Server> = {}): Server =>
  ({ id, label: `srv-${id}`, baseUrl: `https://${id}.test:8765`, token: `token-${id}`, ...extra });
const sess = (name: string, conta?: string): SessionInfo => ({ name, state: 'idle', jsonl: `/j/${name}.jsonl`, conta });
const event = (body: unknown) => parseTagsEvent(JSON.stringify(body));

describe('machineLabel', () => {
  const machine = (name: string) => ({ machine: { name, icon: 'house' as const, color: null } });
  it('etiqueta do servidor vence o nome local, que vence o host', () => {
    expect(machineLabel(server('a'), machine('Casa'))).toBe('Casa');
    expect(machineLabel(server('a'), machine('  '))).toBe('srv-a');
    expect(machineLabel(server('a'), null)).toBe('srv-a');
    expect(machineLabel(server('a', { label: '' }), { machine: null })).toBe('a.test:8765');
  });
});

describe('parseTagsEvent', () => {
  it('dado torto não vira etiqueta; conta torta sai sozinha', () => {
    expect(parseTagsEvent('{')).toBeNull();
    expect(event({ machine_id: 1, machine: null, accounts: {} })).toBeNull();
    expect(event({ machine_id: null, machine: { name: 'x', icon: 'boat', color: null }, accounts: {} })).toBeNull();
    expect(event({ machine_id: 'casa', machine: null, accounts: {
      'claude:/h/.claude': { name: 'claude', icon: 'wallet', color: 'pink' },
      'codex:/h/.codex': { name: 'x', icon: 'credit-card', color: 'red' },
    } })).toEqual({ machine_id: 'casa', machine: null,
      accounts: { 'claude:/h/.claude': { name: 'claude', icon: 'wallet', color: 'pink' } } });
  });
});

describe('evento `tags` na lista', () => {
  const tags = event({
    machine_id: 'casa',
    machine: { name: 'Casa', icon: 'house', color: 'green' },
    accounts: { 'claude:/h/.claude-2': { name: 'claude-200-2', icon: 'wallet', color: 'amber' } },
  })!;

  it('preenche nome, cor e etiquetas só nas linhas do servidor do evento, conta pela `conta` da linha', () => {
    const slots = new Map<string, Slot>([
      ['a', { sessions: [sess('x', 'claude:/h/.claude-2'), sess('y', 'claude:/h/.outra'), sess('z')], error: null, tags }],
      ['b', { sessions: [sess('w', 'claude:/h/.claude-2')], error: null }],
    ]);
    const { rows } = aggregateSessions([server('a'), server('b')], slots);
    const [x, y, z, w] = rows;
    expect(x).toMatchObject({ serverLabel: 'Casa', serverColor: TAG_COLOR_HEX.green,
      machineTag: { name: 'Casa', icon: 'house', color: 'green' },
      accountTag: { name: 'claude-200-2', icon: 'wallet', color: 'amber' } });
    expect(y.accountTag).toBeNull();
    expect(z.accountTag).toBeNull();
    // Servidor sem evento (versão antiga): fica como antes.
    expect(w).toMatchObject({ serverLabel: 'srv-b', serverColor: serverColor('b'), machineTag: null, accountTag: null });
  });

  it('etiqueta sem cor mantém a cor por hash; evento novo refaz as linhas do slot', () => {
    const sessions = [sess('x')];
    const first = aggregateSessions([server('a')], new Map([['a', { sessions, error: null }]])).rows[0];
    const noColor = { ...tags, machine: { ...tags.machine!, color: null } };
    const second = aggregateSessions([server('a')], new Map([['a', { sessions, error: null, tags: noColor }]])).rows[0];
    expect(second).not.toBe(first);
    expect(second).toMatchObject({ serverLabel: 'Casa', serverColor: serverColor('a') });
  });
});

describe('rota /api/tags', () => {
  const fetchMock = vi.fn<typeof fetch>();
  beforeEach(() => {
    _limparEsfriamentoParaTestes();
    fetchMock.mockReset();
    vi.stubGlobal('fetch', fetchMock);
    configureApi({ getBaseUrl: () => 'https://active.test', getToken: () => 'active-token',
      origin: null, onUnauthorized: () => { throw new Error('unexpected-logout'); }, createEventSource: vi.fn() });
  });
  afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); _resetApiEnvForTests(); });

  it('404 = servidor antigo: null, sem erro', async () => {
    fetchMock.mockResolvedValue(new Response('{"detail":"Not Found"}', { status: 404 }));
    expect(await getTagsForServer(server('a'))).toBeNull();
    expect(await putTagsForServer(server('a'), { machine: null })).toBeNull();
    expect(fetchMock.mock.calls.map(([url]) => String(url))).toEqual(['https://a.test:8765/api/tags', 'https://a.test:8765/api/tags']);
  });

  it('PUT leva a mudança no corpo e devolve o documento; outro erro sobe', async () => {
    const doc = { machine: { name: 'Casa', icon: 'house', color: null }, accounts: {} };
    fetchMock.mockResolvedValueOnce(new Response(JSON.stringify(doc), { status: 200 }));
    expect(await putTagsForServer(server('a'), { machine: doc.machine as never })).toEqual(doc);
    expect(fetchMock.mock.calls[0][1]?.method).toBe('PUT');
    expect(JSON.parse(fetchMock.mock.calls[0][1]?.body as string)).toEqual({ machine: doc.machine });
    fetchMock.mockResolvedValueOnce(new Response(JSON.stringify({ detail: { code: 'tags_full' } }), { status: 409 }));
    await expect(putTagsForServer(server('a'), { account: { id: 'x', tag: null } })).rejects.toMatchObject({ status: 409 });
  });
});
