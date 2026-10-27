import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { configureApi, configureDiag } from '@hangar/core';
import type { ChatEvent } from '@hangar/core';
import { chatStore, _resetChatsForTests, filaCount, setChatsForeground, submitConversationDraft, isSubmitting } from './chat';
import * as m from '../paraglide/messages';
import { readDraft, writeDraft } from './drafts';
const disk = vi.hoisted(() => ({ values: new Map<string, string>(), fail: false }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({
  getString: (key: string) => disk.values.get(key),
  set: (key: string, value: string) => { if (disk.fail) throw new Error('disk'); disk.values.set(key, value); },
  remove: (key: string) => { disk.values.delete(key); },
}) }));
const servidores = vi.hoisted(() => ({
  lista: [] as { id: string; label: string; baseUrl: string; token: string }[],
}));
vi.mock('./servers', () => ({ useServers: { getState: () => ({ servers: servidores.lista }) } }));
const toasts = vi.hoisted(() => ({ mod: vi.fn() }));
vi.mock('../ui/Toast', () => ({ toast: toasts }));

// EventSource falso injetado via configureApi (mesmo padrão de sessions.test.ts)
type FakeES = {
  url: string;
  listeners: Record<string, ((e: { data: string; lastEventId?: string }) => void)[]>;
  close: ReturnType<typeof vi.fn>;
  trigger: (type: string, data: string, lastEventId?: string) => void;
  fail: (error?: unknown) => void;
  readyState: number;
  onerror: ((e: unknown) => void) | null;
};

let created: FakeES[] = [];

function fakeCreateEventSource(url: string): unknown {
  const fake: FakeES = {
    url,
    listeners: {},
    close: vi.fn(),
    trigger(type, data, lastEventId) {
      (this.listeners[type] ?? []).forEach((fn) =>
        fn({ data, ...(lastEventId ? { lastEventId } : {}) }),
      );
    },
    fail(error = new Error('tcp')) {
      this.onerror?.(error);
    },
    readyState: 1,
    onerror: null as ((e: unknown) => void) | null,
  };
  created.push(fake);
  return {
    addEventListener(type: string, fn: (e: never) => void) {
      (fake.listeners[type] ??= []).push(fn as never);
    },
    removeEventListener() {},
    close: fake.close,
    // setter do store escreve AQUI; fake.fail() lê daqui
    get onerror() {
      return null;
    },
    set onerror(fn: ((e: unknown) => void) | null) {
      fake.onerror = fn;
    },
    get onopen() {
      return (fake as unknown as { _onopen: ((e: unknown) => void) | null })._onopen ?? null;
    },
    set onopen(fn: ((e: unknown) => void) | null) {
      (fake as unknown as { _onopen: ((e: unknown) => void) | null })._onopen = fn;
      if (fn) (fake.listeners['open'] ??= []).push(fn as never);
    },
    get readyState() {
      return fake.readyState;
    },
  };
}

// fetch falso pro /history — pilha de respostas por ordem de chamada
let historyResponses: ChatEvent[][] = [];
let historyCalls = 0;

function ev(partial: Partial<ChatEvent> & { id: string }): ChatEvent {
  return { kind: 'user_msg', text: partial.id, ts: 1_700_000_000, ...partial } as ChatEvent;
}

beforeEach(() => {
  disk.values.clear();
  disk.fail = false;
  servidores.lista = [
    { id: 'srv1', label: 'um', baseUrl: 'http://10.0.0.1:8765', token: 'tok' },
    { id: 'srv2', label: 'dois', baseUrl: 'http://10.0.0.2:8765', token: 'tok2' },
  ];
  created = [];
  historyCalls = 0;
  historyResponses = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => {
      const body = historyResponses[Math.min(historyCalls, historyResponses.length - 1)] ?? [];
      historyCalls++;
      return new Response(JSON.stringify(body), { status: 200 });
    }),
  );
  configureApi({
    getBaseUrl: () => 'http://10.0.0.1:8765',
    getToken: () => 'tok',
    onUnauthorized: () => {},
    origin: null,
    createEventSource: fakeCreateEventSource as never,
  });
});

afterEach(() => {
  configureDiag({ registrar: () => {}, novoReq: () => '' });
  _resetChatsForTests();
  vi.unstubAllGlobals();
});

test('(a) history inicial + message novo via SSE = N+1 com ids únicos', async () => {
  vi.useFakeTimers();
  try {
    const e1 = ev({ id: 'a:1', kind: 'user_msg', text: 'oi' });
    const e2 = ev({ id: 'a:2', kind: 'assistant_msg', text: 'olá' });
    historyResponses = [[e1, e2]];

    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0); // loadHistory + connectSSE
    expect(chat.use.getState().events).toHaveLength(2);
    expect(created).toHaveLength(1);

    const e3 = ev({ id: 'a:3', kind: 'assistant_msg', text: 'terceiro' });
    created[0].trigger('message', JSON.stringify(e3), 'a:3');
    const events = chat.use.getState().events;
    expect(events).toHaveLength(3);
    expect(new Set(events.map((x) => x.id)).size).toBe(3);

    // retomada: a PRÓXIMA conexão (pós-erro) nasce com last_event_id do último transcript —
    // a primeira nunca tem (?last_event_id só entra na reconexão)
    created[0].fail();
    await vi.advanceTimersByTimeAsync(3_000);
    expect(created).toHaveLength(2);
    expect(created[1].url).toContain('last_event_id=a%3A3');

    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test('ingestão ativa não relê histórico de 5000 eventos para 200 mensagens sem fila', async () => {
  async function measure(size: number) {
    const reads = { id: 0, kind: 0, text: 0 };
    const history = Array.from({ length: size }, (_, i) => new Proxy(ev({
      id: `history:${i}`, kind: i % 2 ? 'assistant_msg' : 'user_msg', text: `histórico ${i}`,
    }), {
      get(target, key, receiver) {
        if (key === 'id' || key === 'kind' || key === 'text') reads[key]++;
        return Reflect.get(target, key, receiver);
      },
    }));
    // Instrumenta o corpo já desserializado na fronteira HTTP, não funções internas do store.
    vi.stubGlobal('fetch', vi.fn(async () => {
      const response = new Response(null, { status: 200 });
      response.json = async () => history;
      return response;
    }));
    const chat = chatStore('srv1', `active-${size}`);
    chat.retain();
    await tick();
    expect(chat.use.getState().events).toHaveLength(size);
    const original = chat.use.getState().events;
    const source = created.at(-1)!;
    reads.id = reads.kind = reads.text = 0;
    for (let i = 0; i < 200; i++) {
      source.trigger('message', JSON.stringify(ev({ id: `live:${i}`, text: `nova ${i}` })));
    }
    const append = { ...reads };
    reads.id = reads.kind = reads.text = 0;
    source.trigger('message', JSON.stringify(ev({ id: 'replace', kind: 'assistant_msg' })));
    for (let i = 0; i < 200; i++) {
      source.trigger('message', JSON.stringify(ev({ id: 'replace', kind: 'assistant_msg', text: `versão ${i}` })));
    }
    const replace = { ...reads };
    reads.id = reads.kind = reads.text = 0;
    vi.useFakeTimers();
    try {
      for (let i = 0; i < 200; i++) {
        source.trigger('preview', JSON.stringify({ text: `prévia ${i}` }));
        source.trigger('state', JSON.stringify({ session: `active-${size}`, state: 'working' }));
      }
      expect(chat.use.getState().preview).toBe('prévia 0');
      await vi.advanceTimersByTimeAsync(50);
      expect(chat.use.getState().preview).toBe('prévia 199');
    } finally {
      vi.clearAllTimers();
      vi.useRealTimers();
    }
    const previewState = { ...reads };
    reads.id = reads.kind = reads.text = 0;
    source.trigger('message', JSON.stringify(ev({ id: 'queued-new', text: 'fila sem correspondente' })));
    // O Proxy não conta comparações com os textos reais cacheados, ainda percorridos pelo sintético.
    const queued = { ...reads };
    const measured = { append, replace, previewState, queued };
    expect(chat.use.getState().events).toHaveLength(size + 202);
    expect(original).toHaveLength(size);
    expect(chat.use.getState().events[0]).toBe(history[0]);
    chat.release();
    return measured;
  }
  const short = await measure(50);
  const long = await measure(5000);
  console.info('ingestão ativa: leituras históricas em 200 mensagens', { short, long });
  for (const reads of Object.values(long)) expect(reads).toEqual({ id: 0, kind: 0, text: 0 });
  expect(long).toEqual(short);
});

test('eventsRevision só conserva a revisão no append e invalida replace, remoção, histórico e reset', async () => {
  historyResponses = [
    [ev({ id: 'tail:1' })],
    [ev({ id: 'older:1' }), ev({ id: 'tail:1' })],
    [ev({ id: 'reset:1' })],
  ];
  const chat = chatStore('srv1', 'sess');
  expect(chat.use.getState().eventsRevision).toBe(0);
  chat.retain();
  await tick();
  let revision = chat.use.getState().eventsRevision;
  expect(revision).toBeGreaterThan(0);
  const source = created[0];
  source.trigger('message', JSON.stringify(ev({ id: 'queued-a', text: 'primeira fila' })));
  source.trigger('message', JSON.stringify(ev({ id: 'tail:2', kind: 'assistant_msg' })));
  expect(chat.use.getState().eventsRevision).toBe(revision);
  const original = chat.use.getState().events;
  source.trigger('message', JSON.stringify(ev({ id: 'tail:2', kind: 'assistant_msg', text: 'atualizada' })));
  expect(chat.use.getState().eventsRevision).toBeGreaterThan(revision);
  expect(original.at(-1)?.text).toBe('tail:2');
  revision = chat.use.getState().eventsRevision;
  source.trigger('queue_confirmed', JSON.stringify(ev({ id: 'queued-a', queued_confirmed: true })));
  expect(chat.use.getState().eventsRevision).toBeGreaterThan(revision);
  revision = chat.use.getState().eventsRevision;
  source.trigger('message', JSON.stringify(ev({ id: 'queued-owner', text: 'entrega real' })));
  source.trigger('message', JSON.stringify(ev({ id: 'real-owner', text: 'entrega real' })));
  expect(chat.use.getState().eventsRevision).toBeGreaterThan(revision);
  expect(chat.use.getState().events.some(e => e.id === 'queued-owner')).toBe(false);
  revision = chat.use.getState().eventsRevision;
  chat.loadOlder();
  await tick();
  expect(chat.use.getState().eventsRevision).toBeGreaterThan(revision);
  expect(chat.use.getState().events[0].id).toBe('older:1');
  revision = chat.use.getState().eventsRevision;
  source.trigger('message', JSON.stringify(ev({ id: 'queued-b', text: 'segunda fila' })));
  source.trigger('queue', JSON.stringify({ entries: [], removed: ['b'], actions: true }));
  expect(chat.use.getState().eventsRevision).toBeGreaterThan(revision);
  revision = chat.use.getState().eventsRevision;
  source.trigger('message', JSON.stringify(ev({ id: 'queued-c', text: 'terceira fila' })));
  chat.applySteer({ queued_ids: ['queued-c'] });
  expect(chat.use.getState().eventsRevision).toBeGreaterThan(revision);
  revision = chat.use.getState().eventsRevision;
  chat.applySteer({ promoted: true });
  expect(chat.use.getState().eventsRevision).toBeGreaterThan(revision);
  revision = chat.use.getState().eventsRevision;
  source.trigger('reset', '{}');
  await tick();
  expect(chat.use.getState().eventsRevision).toBeGreaterThan(revision);
  expect(chat.use.getState().events.map(e => e.id)).toEqual(['reset:1']);
});

test('confirmações antigas da fila não publicam atualizações após carregar o histórico', async () => {
  historyResponses = [[...Array.from({ length: 400 }, (_, i) => ev({
    id: `history:${i}`, kind: i % 2 ? 'assistant_msg' : 'user_msg', text: `histórico ${i}`,
  })), ev({ id: 'queued-current', text: 'aguardando confirmação', queued_delivered: true })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const source = created[0];
  const original = chat.use.getState();
  let updates = 0;
  const unsubscribe = chat.use.subscribe(() => { updates++; });
  const replay = () => {
    for (let i = 0; i < 160; i++) {
      source.trigger('queue_confirmed', JSON.stringify(ev({
        id: `queued-old-${i}`, queued_delivered: true, queued_confirmed: true, queued_ts: 1_700_000_000,
      })));
    }
  };
  try {
    replay();
    replay();
    expect(updates).toBe(0);
    expect(chat.use.getState()).toBe(original);
    source.trigger('queue_confirmed', JSON.stringify(ev({ id: 'queued-current', queued_confirmed: true })));
    expect(updates).toBe(1);
    expect(chat.use.getState().events).toHaveLength(400);
    expect(chat.use.getState().events.some(e => e.id === 'queued-current')).toBe(false);
    const confirmed = chat.use.getState();
    source.trigger('queue_confirmed', JSON.stringify(ev({ id: 'queued-current', queued_confirmed: true })));
    replay();
    expect(updates).toBe(1);
    expect(chat.use.getState()).toBe(confirmed);
    source.trigger('message', JSON.stringify(ev({ id: 'history:399', kind: 'assistant_msg', text: 'conteúdo atualizado' })));
    source.trigger('message', JSON.stringify(ev({ id: 'new:1', kind: 'assistant_msg', text: 'mensagem nova' })));
    expect(chat.use.getState().events.slice(-2).map(e => e.text)).toEqual(['conteúdo atualizado', 'mensagem nova']);
    expect(chat.use.getState().events[0]).toBe(original.events[0]);
    expect(original.events).toHaveLength(401);
  } finally {
    unsubscribe();
    chat.release();
  }
});

test('confirmação retira apenas o eco da fila e preserva índices no próximo evento', async () => {
  historyResponses = [[ev({ id: 'queued-a' }), ev({ id: 'real' }), ev({ id: 'queued-b' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('queue_confirmed', JSON.stringify(ev({ id: 'queued-a', queued_confirmed: true })));
  expect(chat.use.getState().events.map(e => e.id)).toEqual(['real', 'queued-b']);
  created[0].trigger('message', JSON.stringify(ev({ id: 'real', text: 'atualizado' })));
  expect(chat.use.getState().events.map(e => e.id)).toEqual(['real', 'queued-b']);
  expect(chat.use.getState().events[0].text).toBe('atualizado');
  created[0].trigger('queue_confirmed', JSON.stringify(ev({ id: 'queued-a', queued_confirmed: true })));
  expect(chat.use.getState().events).toHaveLength(2);
});

test('mods recebem retrato e delta, preservam ordem e limpam no reset', async () => {
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const first = { id: 'first', title: 'first', tree: { type: 'Text', children: ['one'] } };
  const second = { id: 'second', tree: { type: 'Text', children: ['two'] } };
  created[0].trigger('plugin_ui', JSON.stringify({ above: { type: 'Text', children: ['band'] }, panes: [first, second], source: 'surface' }));
  expect(chat.use.getState().pluginUi?.panes.map(p => p.id)).toEqual(['first', 'second']);
  created[0].trigger('plugin_ui_delta', JSON.stringify({ panes: [{ id: 'second', same: true }], source: 'surface', shown_id: 'second' }));
  expect(chat.use.getState().pluginUi?.above).toEqual({ type: 'Text', children: ['band'] });
  expect(chat.use.getState().pluginUi?.panes.map(p => p.id)).toEqual(['second']);
  expect(chat.use.getState().pluginUi?.panes[0].tree).toEqual(second.tree);
  expect(chat.use.getState().pluginUi?.shownId).toBe('second');
  created[0].trigger('plugin_ui_delta', JSON.stringify({ above: null, panes: [] }));
  expect(chat.use.getState().pluginUi?.above).toBeNull();
  expect(chat.use.getState().pluginUi?.panes).toEqual([]);
  created[0].trigger('reset', '');
  expect(chat.use.getState().pluginUi).toBeNull();
  expect(chat.use.getState().pluginUiError).toBeNull();
});

test('delta de mod sem base desta conexão expõe erro, conserva vista e reconecta', async () => {
  vi.useFakeTimers();
  try {
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);
    created[0].trigger('plugin_ui', JSON.stringify({ above: { type: 'Text', children: ['saved'] }, panes: [] }));
    const saved = chat.use.getState().pluginUi;
    created[0].fail();
    await vi.advanceTimersByTimeAsync(3000);
    created[1].trigger('plugin_ui_delta', JSON.stringify({ panes: [] }));
    expect(chat.use.getState().pluginUi).toBe(saved);
    expect(chat.use.getState().pluginUiError).toBeTruthy();
    expect(created[1].close).toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(6000);
    expect(created.length).toBeGreaterThanOrEqual(3);
    created.at(-1)!.trigger('plugin_ui', JSON.stringify({ above: null, panes: [] }));
    expect(chat.use.getState().pluginUiError).toBeNull();
    expect(chat.use.getState().pluginUi?.above).toBeNull();
  } finally {
    vi.useRealTimers();
  }
});

test('mods são isolados por servidor e ignoram quadros de fonte fechada', async () => {
  const first = chatStore('srv1', 'sess');
  const other = chatStore('srv2', 'sess');
  first.retain();
  other.retain();
  await tick();
  created[0].trigger('plugin_ui', JSON.stringify({ above: { type: 'Text', children: ['first'] }, panes: [] }));
  expect(other.use.getState().pluginUi).toBeNull();
  first.release();
  created[0].trigger('plugin_ui', JSON.stringify({ above: null, panes: [] }));
  expect(first.use.getState().pluginUi?.above).toEqual({ type: 'Text', children: ['first'] });
});

test('(b) reset zera tudo e recarrega o history novo', async () => {
  historyResponses = [
    [ev({ id: 'old:1' })],
    [ev({ id: 'new:1' })],
  ];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  expect(chat.use.getState().events[0]?.id).toBe('old:1');

  created[0].trigger('message', JSON.stringify(ev({ id: 'old:2' })), 'old:2');
  expect(chat.use.getState().events).toHaveLength(2);

  created[0].trigger('reset', '{}');
  await tick();
  const s = chat.use.getState();
  expect(s.events).toHaveLength(1);
  expect(s.events[0]?.id).toBe('new:1');
  expect(s.stateEvent).toBeNull();
  expect(s.statusLine).toBeNull();
  expect(s.loading).toBe(false);
  // id do transcript antigo não sobrevive ao reset
  expect(created[0].url).not.toContain('last_event_id=old%3A2');
});

test.each(['claude', 'codex'])('reset do %s apaga a conversa e o eco de /clear antes do histórico responder', async (provider) => {
  historyResponses = [[ev({ id: 'old:1', kind: 'assistant_msg', text: 'Conversa anterior' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const source = created[0];
  source.trigger('state', JSON.stringify({ session: 'sess', provider, state: 'working' }));
  source.trigger('preview', JSON.stringify({ text: 'Prévia anterior', md: true }));
  chat.use.setState({ pending: [{ id: 'clear', text: ' /clear ' }, { id: 'next', text: 'Mensagem aguardando' }] });
  const requests = fetchManual();

  source.trigger('reset', '{}');
  expect(chat.use.getState()).toMatchObject({ events: [], preview: '', stateEvent: null, loading: true,
    pending: [{ id: 'next', text: 'Mensagem aguardando' }] });
  // O índice antigo também sai: o primeiro evento da conversa nova pode reutilizar um ID.
  source.trigger('message', JSON.stringify(ev({ id: 'old:1', text: 'Mensagem nova' })));
  expect(chat.use.getState().events.map((e) => e.text)).toEqual(['Mensagem nova']);
  requests[0].responder(null, { status: 503 });
  await tick();
  expect(chat.use.getState().events.map((e) => e.text)).toEqual(['Mensagem nova']);
  expect(chat.use.getState().loading).toBe(false);
  expect(chat.use.getState().error).not.toBe('');
  chat.release();
});

test.each([
  { tail: [], live: [ev({ id: 'new:1', text: 'Ao vivo' })], texts: ['Ao vivo'] },
  { tail: [ev({ id: 'new:0', text: 'Anterior' })], live: [ev({ id: 'new:1', text: 'Ao vivo' })], texts: ['Anterior', 'Ao vivo'] },
  { tail: [ev({ id: 'new:1', text: 'Versão antiga' })], live: [ev({ id: 'new:1', text: 'Versão atual' }), ev({ id: 'new:2', text: 'Seguinte' })],
    texts: ['Versão atual', 'Seguinte'] },
])('histórico após reset preserva as mensagens SSE: $texts', async ({ tail, live, texts }) => {
  historyResponses = [[ev({ id: 'old:1', text: 'Conversa anterior' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const requests = fetchManual();
  created[0].trigger('reset', '{}');
  for (const message of live) created[0].trigger('message', JSON.stringify(message));
  requests[0].responder(tail);
  await tick();
  expect(chat.use.getState().events.map((e) => e.text)).toEqual(texts);
  expect(chat.use.getState().loading).toBe(false);
  chat.release();
});

test('histórico após reset confirma só a fila correspondente entre os eventos ao vivo', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const requests = fetchManual();
  created[0].trigger('reset', '{}');
  const common = ev({ id: 'common', kind: 'assistant_msg', text: 'Evento comum' });
  for (const message of [common, ev({ id: 'queued-x', text: 'Primeira mensagem' }), ev({ id: 'queued-y', text: 'Outra mensagem' })]) {
    created[0].trigger('message', JSON.stringify(message));
  }
  requests[0].responder([common, ev({ id: 'real', text: 'Primeira mensagem' })]);
  await tick();
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['common', 'real', 'queued-y']);
  chat.release();
});

test.each(['real', 'confirmed', 'removed'])('histórico atrasado e replay não devolvem fila retirada por %s', async (reason) => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const requests = fetchManual();
  const source = created[0];
  source.trigger('reset', '{}');
  const common = ev({ id: 'common', kind: 'assistant_msg' });
  const queued = ev({ id: 'queued-x', text: 'Primeira mensagem' });
  for (const message of [common, queued, ev({ id: 'queued-y', text: 'Outra mensagem' })]) {
    source.trigger('message', JSON.stringify(message));
  }
  if (reason === 'real') source.trigger('message', JSON.stringify(ev({ id: 'real', text: queued.text })));
  else if (reason === 'confirmed') source.trigger('queue_confirmed', JSON.stringify({ ...queued, queued_confirmed: true }));
  else source.trigger('queue', JSON.stringify({ entries: [], removed: ['x'], actions: true }));
  requests[0].responder([common, queued]);
  await tick();
  const ids = reason === 'real' ? ['common', 'queued-y', 'real'] : ['common', 'queued-y'];
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(ids);
  source.trigger('message', JSON.stringify(queued));
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(ids);
  source.trigger('reset', '{}');
  source.trigger('message', JSON.stringify(queued));
  requests[1].responder([]);
  await tick();
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['queued-x']);
  chat.release();
});

test('/clear aceito só retira seu eco quando o servidor confirma a troca de conversa', async () => {
  historyResponses = [[ev({ id: 'old:1', kind: 'assistant_msg', text: 'Conversa anterior' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const requests = fetchManual();
  const sending = chat.send('/clear');
  expect(chat.use.getState().events).toHaveLength(1);
  requests[0].responder(null);
  await sending;
  expect(chat.use.getState().pending.map((p) => p.text)).toEqual(['/clear']);

  created[0].trigger('reset', '{}');
  requests[1].responder([]);
  await tick();
  expect(chat.use.getState().events).toEqual([]);
  expect(chat.use.getState().pending).toEqual([]);
  chat.release();
});

test('(c) preview some quando o assistant_msg real chega', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();

  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
  created[0].trigger('preview', JSON.stringify({ text: 'pensando…', md: true, full: true }));
  expect(chat.use.getState().preview).toBe('pensando…');

  created[0].trigger('preview', JSON.stringify({ text: 'pensando mais', md: true, full: true }));
  await new Promise((r) => setTimeout(r, 60));
  expect(chat.use.getState().preview).toBe('pensando mais');

  created[0].trigger(
    'message',
    JSON.stringify(ev({ id: 'm:1', kind: 'assistant_msg', text: 'resposta final' })),
  );
  expect(chat.use.getState().preview).toBe('');
  expect(chat.use.getState().events).toHaveLength(1);
});

test('(c2) preview vazio durante working NÃO apaga a bolha; sair de working apaga após a carência', async () => {
  vi.useFakeTimers();
  try {
    historyResponses = [[]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);

    created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
    created[0].trigger('preview', JSON.stringify({ text: 'rascunho' }));
    expect(chat.use.getState().preview).toBe('rascunho');

    // entre ferramentas o extrator manda "" — bolha fica
    created[0].trigger('preview', JSON.stringify({ text: '' }));
    expect(chat.use.getState().preview).toBe('rascunho');

    // fim do turno: a prévia espera o bloco real em vez de piscar
    created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'idle' }));
    await vi.advanceTimersByTimeAsync(4_000);
    expect(chat.use.getState().preview).toBe('rascunho');
    await vi.advanceTimersByTimeAsync(1_000);
    expect(chat.use.getState().preview).toBe('');

    // turno novo antes da carência vencer cancela a saída
    created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
    created[0].trigger('preview', JSON.stringify({ text: 'outro' }));
    created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'idle' }));
    created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
    await vi.advanceTimersByTimeAsync(6_000);
    expect(chat.use.getState().preview).toBe('outro');
    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test('pensamento e ferramenta ao vivo: o bloco real tira de cena; o "" só agenda', async () => {
  vi.useFakeTimers();
  try {
    historyResponses = [[]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);

    created[0].trigger('pensamento', JSON.stringify({ text: 'pondero' }));
    created[0].trigger('ferramenta', JSON.stringify({ text: JSON.stringify({ nome: 'Bash', input: { command: 'ls' } }) }));
    expect(chat.use.getState().pensamento).toBe('pondero');
    expect(chat.use.getState().ferramenta).toEqual({ nome: 'Bash', input: { command: 'ls' } });

    created[0].trigger('message', JSON.stringify(ev({ id: 't:1', kind: 'thinking', text: 'pondero' })));
    expect(chat.use.getState().pensamento).toBe('');
    created[0].trigger('ferramenta', JSON.stringify({ text: '' }));
    expect(chat.use.getState().ferramenta).not.toBeNull();
    await vi.advanceTimersByTimeAsync(3_000);
    expect(chat.use.getState().ferramenta).toBeNull();
    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test('aviso de mod vira toast uma vez só, mesmo reposto pela reconexão', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await vi.waitFor(() => expect(created).toHaveLength(1));

  const frame = JSON.stringify({ id: 'ab-1', text: 'Jenkins configurado.', plugin: 'demo', timeoutMs: 9000 });
  created[0].trigger('plugin_toast', frame);
  created[0].trigger('plugin_toast', frame);
  created[0].trigger('plugin_toast', JSON.stringify({ text: 'sem id' }));

  await vi.waitFor(() => expect(toasts.mod).toHaveBeenCalled());
  expect(toasts.mod.mock.calls).toEqual([['Jenkins configurado.', 'demo', 9000]]);
  chat.release();
});

test('pensamento e ferramenta ao vivo saem quando o estado deixa working sem o evento vazio', async () => {
  vi.useFakeTimers();
  try {
    historyResponses = [[]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);

    created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
    created[0].trigger('pensamento', JSON.stringify({ text: 'pondero' }));
    created[0].trigger('ferramenta', JSON.stringify({ text: JSON.stringify({ nome: 'Bash', input: {} }) }));
    created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'idle' }));
    expect(chat.use.getState().pensamento).toBe('pondero');
    await vi.advanceTimersByTimeAsync(3_000);
    expect(chat.use.getState().pensamento).toBe('');
    expect(chat.use.getState().ferramenta).toBeNull();
    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test('virada para working vista ao vivo marca o começo do turno; aberta no meio, não', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
  expect(chat.use.getState().turnSeen).toBeNull();
  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'idle' }));
  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
  expect(chat.use.getState().turnSeen).toEqual(expect.any(Number));
});

test('replay idêntico conserva referências; metadados e conteúdo novos ainda substituem', async () => {
  historyResponses = [[ev({ id: 'a:1', kind: 'assistant_msg', text: 'igual', cache_read: 10 })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const original = chat.use.getState();
  const listener = vi.fn();
  const unsubscribe = chat.use.subscribe(listener);
  try {
    created[0].trigger('message', JSON.stringify(original.events[0]));
    expect(chat.use.getState()).toBe(original);
    expect(listener).not.toHaveBeenCalled();
    created[0].trigger('message', JSON.stringify({ ...original.events[0], cache_read: 20 }));
    expect(chat.use.getState().events[0].cache_read).toBe(20);
    expect(chat.use.getState().eventsRevision).toBeGreaterThan(original.eventsRevision);
    created[0].trigger('message', JSON.stringify(ev({ id: 'a:1', kind: 'assistant_msg', text: 'cresceu' })));
    expect(chat.use.getState().events[0].text).toBe('cresceu');
    expect(chat.use.getState().events[0]).not.toHaveProperty('cache_read');
    expect(original.events[0]).toMatchObject({ text: 'igual', cache_read: 10 });
  } finally {
    unsubscribe();
  }
});

test.each([
  ev({ id: 'tool', kind: 'tool_use', tool_input: { command: 'ls', options: { paths: ['src'] } } }),
  ev({ id: 'result', kind: 'tool_result', result: 'ok', patch: [{ old_start: 1, new_start: 1, lines: ['+linha'] }] }),
])('replay estruturado idêntico conserva referências e revisão: $kind', async (event) => {
  historyResponses = [[event]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const original = chat.use.getState();
  const listener = vi.fn();
  const unsubscribe = chat.use.subscribe(listener);
  try {
    created[0].trigger('message', JSON.stringify(event));
    expect(chat.use.getState()).toBe(original);
    expect(listener).not.toHaveBeenCalled();
    const changed = event.kind === 'tool_use'
      ? { ...event, tool_input: { command: 'ls', options: { paths: ['other'] } } }
      : { ...event, patch: [{ old_start: 1, new_start: 1, lines: ['+outra linha'] }] };
    created[0].trigger('message', JSON.stringify(changed));
    expect(chat.use.getState().events[0]).toEqual(changed);
    expect(chat.use.getState().eventsRevision).toBeGreaterThan(original.eventsRevision);
    const removed = ev({ id: event.id, kind: event.kind });
    created[0].trigger('message', JSON.stringify(removed));
    expect(chat.use.getState().events[0]).toEqual(removed);
    expect(original.events[0]).toEqual(event);
  } finally {
    unsubscribe();
  }
});

test('(d) message duplicado (mesmo id) não entra; conteúdo novo substitui', async () => {
  historyResponses = [[ev({ id: 'a:1', text: 'v1' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();

  // replay do SSE re-emitindo o MESMO id com texto diferente -> substitui, não duplica
  created[0].trigger('message', JSON.stringify(ev({ id: 'a:1', text: 'v2' })));
  let events = chat.use.getState().events;
  expect(events).toHaveLength(1);
  expect(events[0]?.text).toBe('v2');

  created[0].trigger('message', JSON.stringify(ev({ id: 'a:1', text: 'v3' })));
  events = chat.use.getState().events;
  expect(events).toHaveLength(1);
  expect(events[0]?.text).toBe('v3');
});

test('state atualiza statusLine; release fecha o stream', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();

  created[0].trigger(
    'state',
    JSON.stringify({ session: 'sess', state: 'working', status_line: '🤖 modelo' }),
  );
  expect(chat.use.getState().statusLine).toBe('🤖 modelo');

  chat.release();
  expect(created[0].close).toHaveBeenCalled();
});

test('aviso do Codex acompanha o estado sem virar pergunta', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working', codex_buffering: true }));
  expect(chat.use.getState().stateEvent?.codex_buffering).toBe(true);
  expect(chat.use.getState().stateEvent?.state).toBe('working');
  expect(chat.use.getState().askOpen).toBe(false);
  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working', codex_buffering: false }));
  expect(chat.use.getState().stateEvent?.codex_buffering).toBe(false);
  chat.release();
});

test('loadOlder prependa o histórico antigo; sem costura marca unjoinable', async () => {
  historyResponses = [
    [ev({ id: 't:5' }), ev({ id: 't:6' })],
    [ev({ id: 't:1' }), ev({ id: 't:2' }), ev({ id: 't:5' })],
  ];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  expect(chat.use.getState().events).toHaveLength(2);

  chat.loadOlder();
  await tick();
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['t:1', 't:2', 't:5', 't:6']);
  expect(chat.use.getState().olderFailed).toBe('');

  // segunda busca sem nenhum id em comum -> costura quebrada, avisa
  historyCalls = 99; // fetch devolve a última resposta configurada
  historyResponses[1] = [ev({ id: 'outro:9' })];
  chat.loadOlder();
  await tick();
  expect(chat.use.getState().olderFailed).toBe('unjoinable');
});

test('(e) user_msg da fila (queued-) sai quando o real chega com o mesmo texto', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();

  // cp-send enfileira: backend emite o sintético ANTES do transcript gravar o real
  created[0].trigger(
    'message',
    JSON.stringify(ev({ id: 'queued-42', text: 'oi tudo bem' })),
  );
  expect(chat.use.getState().events).toHaveLength(1);

  // prompt real commitado, texto igual: a bolha sintética é substituída pela real
  created[0].trigger('message', JSON.stringify(ev({ id: 'a:9', text: 'oi tudo bem' })));
  const events = chat.use.getState().events;
  expect(events).toHaveLength(1);
  expect(events[0]?.id).toBe('a:9');
});

test.each([
  { queue: ['Vamos fazer', 'Vamos fazer hoje'], real: 'Vamos fazer hoje', remaining: 'queued-0' },
  { queue: ['Vamos fazer', 'Vamos fazer hoje'], real: 'Vamos fazer hoje agora', remaining: 'queued-0' },
  { queue: ['Vamos fazer', 'Vamos fazer hoje — 📎 imagem: /tmp/foto.png'], real: 'Vamos fazer hoje', remaining: 'queued-0' },
  { queue: ['igual', 'igual'], real: 'igual', remaining: 'queued-1' },
])('dedup preserva dono e especificidade: $real / $queue', async ({ queue, real, remaining }) => {
  historyResponses = [queue.map((text, i) => ev({ id: `queued-${i}`, text }))];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('message', JSON.stringify(ev({ id: 'real', text: real })));
  expect(chat.use.getState().events.map(e => e.id)).toEqual([remaining, 'real']);
});

test('sintético respeita fila mais específica antes de considerar a cobertura pelo real', async () => {
  historyResponses = [[ev({ id: 'real', text: 'Vamos fazer hoje agora' }),
    ev({ id: 'queued-specific', text: 'Vamos fazer hoje' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('message', JSON.stringify(ev({ id: 'queued-short', text: 'Vamos fazer' })));
  created[0].trigger('message', JSON.stringify(ev({ id: 'queued-exact', text: 'Vamos fazer hoje agora' })));
  expect(chat.use.getState().events.map(e => e.id)).toEqual(['real', 'queued-specific', 'queued-short']);
});

test('índices de candidatos acompanham substituição de texto e kind sem mudar desempate por posição', async () => {
  historyResponses = [[ev({ id: 'queued-first', text: 'texto antigo' }),
    ev({ id: 'queued-second', text: 'texto igual' }), ev({ id: 'real', text: 'texto conhecido' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const source = created[0];
  source.trigger('message', JSON.stringify(ev({ id: 'queued-first', text: 'texto igual' })));
  source.trigger('message', JSON.stringify(ev({ id: 'confirmed', text: 'texto igual' })));
  expect(chat.use.getState().events.map(e => e.id)).toEqual(['queued-second', 'real', 'confirmed']);
  // O texto real indexado cobre o sintético, até a linha deixar de ser user_msg.
  source.trigger('message', JSON.stringify(ev({ id: 'queued-specific', text: 'texto conhecido' })));
  expect(chat.use.getState().events.some(e => e.id === 'queued-specific')).toBe(false);
  source.trigger('message', JSON.stringify(ev({ id: 'real', kind: 'assistant_msg', text: 'texto conhecido' })));
  source.trigger('message', JSON.stringify(ev({ id: 'queued-specific', text: 'texto conhecido' })));
  expect(chat.use.getState().events.at(-1)?.id).toBe('queued-specific');
  source.trigger('message', JSON.stringify(ev({ id: 'real', text: 'texto conhecido com sufixo' })));
  source.trigger('message', JSON.stringify(ev({ id: 'queued-short', text: 'texto conhecido' })));
  expect(chat.use.getState().events.some(e => e.id === 'queued-short')).toBe(false);
});

test.each([false, true, 'identical'] as const)('real já conhecido: %s — remover a fila dona preserva os vizinhos no replay SSE', async (known) => {
  const before = ev({ id: 'a:1', kind: 'assistant_msg', text: 'antes' });
  const neighbor = ev({ id: 'a:2', kind: 'assistant_msg', text: 'vizinho' });
  const otherQueue = ev({ id: 'queued-other', text: 'outra mensagem' });
  const real = ev({ id: 'a:3', text: 'mensagem confirmada' });
  historyResponses = [[before, ev({ id: 'queued-owner', text: real.text }), neighbor, otherQueue,
    ...(known ? [{ ...real, text: known === 'identical' ? real.text : 'texto parcial' }] : [])]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();

  created[0].trigger('message', JSON.stringify(real));
  expect(chat.use.getState().events).toEqual([before, neighbor, otherQueue, real]);

  const updatedNeighbor = { ...neighbor, text: 'vizinho atualizado' };
  created[0].trigger('message', JSON.stringify(updatedNeighbor));
  expect(chat.use.getState().events).toEqual([before, updatedNeighbor, otherQueue, real]);
  created[0].trigger('message', JSON.stringify(otherQueue));
  created[0].trigger('message', JSON.stringify(before));
  created[0].trigger('message', JSON.stringify(real));
  expect(chat.use.getState().events).toEqual([before, updatedNeighbor, otherQueue, real]);
  chat.release();
});

test('(f) filaCount conta só queued-* (o eco em voo não é fila) e zera quando o real chega', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const origFetch = global.fetch;
  // send() usa sendInput -> fetch POST; stub pra sucesso
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{}', { status: 200 })));
  const p = chat.send('oi tudo bem');
  // eco local já entrou
  expect(chat.use.getState().pending).toHaveLength(1);
  // Sem estado ainda: o eco nasce atenuado, mas o POST em voo não acende o chip.
  expect(chat.use.getState().pending[0].solid).toBe(false);
  expect(filaCount(chat.use.getState(), 'kimi')).toBe(0);
  expect(filaCount(chat.use.getState(), 'claude')).toBe(0);
  // sintético queued-* chega com mesmo texto -> pending reconciliado, queued entra
  // restaura fetch falso de history pra não quebrar o SSE trigger, mas mantém send mock
  // o trigger não depende de fetch, só do store
  created[0].trigger('message', JSON.stringify(ev({ id: 'queued-1', text: 'oi tudo bem' })));
  await tick();
  expect(chat.use.getState().pending).toHaveLength(0);
  expect(filaCount(chat.use.getState(), 'kimi')).toBe(1);
  // render queued translúcido: events contém queued-*
  expect(chat.use.getState().events.some((e) => e.id.startsWith('queued-'))).toBe(true);
  // real chega -> queued sai, fila zera
  created[0].trigger('message', JSON.stringify(ev({ id: 'a:10', text: 'oi tudo bem' })));
  await tick();
  expect(filaCount(chat.use.getState(), 'kimi')).toBe(0);
  expect(chat.use.getState().events.some((e) => e.id.startsWith('queued-'))).toBe(false);
  await p;
  vi.stubGlobal('fetch', origFetch as never);
});

test('evento queue alimenta o painel e o reset o limpa', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('queue', JSON.stringify({ entries: [{ id: 'a', text: 'oi', ts: 1 }], actions: true }));
  expect(chat.use.getState().queuePanel?.entries.map((e) => e.id)).toEqual(['a']);
  created[0].trigger('reset', '{}');
  expect(chat.use.getState().queuePanel).toBeNull();
});

test('a apagada noutro aparelho tira a bolha daqui', async () => {
  historyResponses = [[ev({ id: 'queued-a', text: 'oi', queued_delivered: false }), ev({ id: 'queued-b', text: 'tchau', queued_delivered: false })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('queue', JSON.stringify({ entries: [{ id: 'b', text: 'tchau', ts: 2 }], actions: true, removed: ['a'] }));
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['queued-b']);
  expect(chat.use.getState().queuePanel?.entries.map((e) => e.id)).toEqual(['b']);
});

test('conexão aberta sem o evento queue volta o painel às bolhas', async () => {
  vi.useFakeTimers();
  try {
    historyResponses = [[]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);
    created[0].trigger('queue', JSON.stringify({ entries: [{ id: 'a', text: 'oi', ts: 1 }], actions: true }));
    created[0].trigger('open', '{}');
    await vi.advanceTimersByTimeAsync(5_000);
    expect(chat.use.getState().queuePanel?.entries.map((e) => e.id)).toEqual(['a']);
    // Reconectou e o retrato veio sem `queue` (a sessão voltou ao Python).
    created[0].fail();
    await vi.advanceTimersByTimeAsync(3_000);
    created[1].trigger('open', '{}');
    await vi.advanceTimersByTimeAsync(5_000);
    expect(chat.use.getState().queuePanel).toBeNull();
    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test('editar guarda o rascunho uma vez e cancelar o devolve', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('queue', JSON.stringify({ entries: [{ id: 'a', text: 'oi', ts: 1 }, { id: 'b', text: 'tchau', ts: 2 }], actions: true }));
  expect(chat.startQueueEdit('a', 'meu rascunho')).toBe('oi');
  expect(chat.startQueueEdit('b', 'oi mudado')).toBe('tchau');
  expect(chat.use.getState().queueEdit).toMatchObject({ entryId: 'b', savedDraft: 'meu rascunho' });
  chat.cancelQueueEdit();
  // Sem texto de saída, o Composer devolve o rascunho guardado.
  expect(chat.use.getState().queueEdit).toBeNull();
  expect(chat.use.getState().queueEditExit).toBeNull();
});

test('ações da fila vão ao servidor da conversa; Atualizar igual só sai da edição e apagar tira a bolha', async () => {
  historyResponses = [[ev({ id: 'queued-a', text: 'primeira', queued_delivered: false })]];
  const chat = chatStore('srv2', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('queue', JSON.stringify({ entries: [{ id: 'a', text: 'primeira', ts: 1 }, { id: 'b', text: 'segunda', ts: 2 }], actions: true }));
  const calls: { url: string; method?: string; body?: unknown }[] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), method: init?.method, body: init?.body ? JSON.parse(init.body as string) : undefined });
    return new Response('{"ok":true,"steered":true}', { status: 200 });
  }));

  chat.startQueueEdit('a', 'rascunho');
  await chat.submitQueueEdit(' primeira ');
  expect(calls).toHaveLength(0);
  expect(chat.use.getState().queueEdit).toBeNull();
  expect(chat.use.getState().queueEditExit).toBeNull();

  chat.startQueueEdit('a', 'rascunho');
  await chat.submitQueueEdit('primeira mudada');
  expect(chat.use.getState().queueEdit).toBeNull();
  expect(chat.use.getState().queueEditExit).toBeNull();
  await chat.moveQueued('a', 1);
  await chat.moveQueued('a', -1); // já é a primeira: nada a pedir
  await chat.steerQueued('b');
  await chat.removeQueued('a');
  expect(calls.map((c) => [c.method, c.url.replace('http://10.0.0.2:8765', ''), c.body])).toEqual([
    ['PATCH', '/api/sessions/sess/queue/a', { text: 'primeira mudada' }],
    ['POST', '/api/sessions/sess/queue/reorder', { id: 'a', before_id: null }],
    ['POST', '/api/sessions/sess/queue/b/steer', undefined],
    ['DELETE', '/api/sessions/sess/queue/a?only_waiting=true', undefined],
  ]);
  expect(chat.use.getState().events.some((e) => e.id === 'queued-a')).toBe(false);
  expect(chat.use.getState().queueBusy).toBe(false);
});

test('descartar usa ID cru e servidor da rota, espera ACK e recompõe o índice sem tirar vizinhos', async () => {
  configureApi({ getBaseUrl: () => 'http://10.0.0.2:8765', getToken: () => 'tok2', onUnauthorized: () => {},
    origin: null, createEventSource: fakeCreateEventSource as never });
  historyResponses = [[ev({ id: 'queued-raw/id', text: 'incerta', queued_delivered: true }),
    ev({ id: 'raw/id', text: 'real' }), ev({ id: 'queued-b', text: 'outra' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const requests = fetchManual();
  const discard = chat.discardQueued('raw/id');
  expect(requests).toHaveLength(1);
  expect(requests[0].url).toBe('http://10.0.0.1:8765/api/sessions/sess/queue/raw%2Fid');
  expect(requests[0].init?.method).toBe('DELETE');
  expect(new Headers(requests[0].init?.headers).get('Authorization')).toBe('Bearer tok');
  expect(chat.use.getState().queueBusy).toBe(true);
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['queued-raw/id', 'raw/id', 'queued-b']);
  expect(await chat.discardQueued('b')).toBeNull();
  expect(requests).toHaveLength(1);
  requests[0].responder({ ok: true });
  expect(await discard).toBeNull();
  expect(chat.use.getState().queueBusy).toBe(false);
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['raw/id', 'queued-b']);
  created[0].trigger('message', JSON.stringify(ev({ id: 'queued-b', text: 'outra atualizada' })));
  created[0].trigger('message', JSON.stringify(ev({ id: 'raw/id', text: 'real atualizado' })));
  expect(chat.use.getState().events.map((e) => [e.id, e.text])).toEqual([
    ['raw/id', 'real atualizado'], ['queued-b', 'outra atualizada'],
  ]);
});

test('descarte recusado conserva a bolha e mostra a causa; nova tentativa só sai no sucesso', async () => {
  historyResponses = [[ev({ id: 'queued-a', text: 'perdida', desistiu: true })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{"detail":"entrega ainda em análise"}', { status: 409 })));
  expect(await chat.discardQueued('a')).toBe('entrega ainda em análise');
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['queued-a']);
  expect(chat.use.getState().queueError).toBe('entrega ainda em análise');
  expect(chat.use.getState().queueBusy).toBe(false);
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{"ok":true}', { status: 200 })));
  expect(await chat.discardQueued('a')).toBeNull();
  expect(chat.use.getState().events).toEqual([]);
  expect(chat.use.getState().queueError).toBeNull();
});

test('descarte com servidor removido mostra erro e não chama o servidor ativo', async () => {
  const chat = chatStore('sumiu', 'sess');
  const lost = ev({ id: 'queued-a', desistiu: true });
  chat.use.setState({ events: [lost] });
  const requests = fetchManual();
  expect(await chat.discardQueued('a')).toBe(m.chat_servidor_removido());
  expect(requests).toHaveLength(0);
  expect(chat.use.getState().events).toEqual([lost]);
  expect(chat.use.getState().queueError).toBe(m.chat_servidor_removido());
  expect(chat.use.getState().queueBusy).toBe(false);
});

test('descarte retorna a própria causa formatada mesmo se outro aviso substituir queueError antes do retorno', async () => {
  const chat = chatStore('srv1', 'sess');
  const lost = ev({ id: 'queued-a', desistiu: true });
  chat.use.setState({ events: [lost] });
  vi.stubGlobal('fetch', vi.fn(async () => new Response(
    JSON.stringify({ detail: { code: 'queue_entry_gone', params: {}, msg: 'x' } }), { status: 409 })));
  const unsubscribe = chat.use.subscribe((state, previous) => {
    if (previous.queueBusy && !state.queueBusy) chat.use.setState({ queueError: 'other-action' });
  });
  try {
    expect(await chat.discardQueued('a')).toBe(m.queue_entry_gone());
    expect(chat.use.getState().queueError).toBe('other-action');
    expect(chat.use.getState().events).toEqual([lost]);
    expect(chat.use.getState().queueBusy).toBe(false);
  } finally {
    unsubscribe();
  }
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{"detail":"next-error"}', { status: 409 })));
  expect(await chat.discardQueued('a')).toBe('next-error');
  expect(chat.use.getState().queueError).toBe('next-error');
  expect(chat.use.getState().events).toEqual([lost]);
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{"ok":true}', { status: 200 })));
  expect(await chat.discardQueued('a')).toBeNull();
  expect(chat.use.getState().queueError).toBeNull();
  expect(chat.use.getState().events).toEqual([]);
});

test('recusa da fila vira aviso traduzido uma vez e a edição da que saiu fecha', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('queue', JSON.stringify({ entries: [{ id: 'a', text: 'oi', ts: 1 }], actions: true }));
  vi.stubGlobal('fetch', vi.fn(async () => new Response(
    JSON.stringify({ detail: { code: 'queue_entry_gone', params: {}, msg: 'x' } }), { status: 409 })));
  chat.startQueueEdit('a', '');
  await chat.submitQueueEdit('oi de novo');
  // Fechada aqui, o Composer não vê a saída pelo painel e não avisa de novo.
  expect(chat.use.getState().queueEdit).toBeNull();
  // Rascunho guardado vazio: o texto editado fica no campo, não some.
  expect(chat.use.getState().queueEditExit).toBe('oi de novo');
  expect(chat.use.getState().queueError).toBe(m.queue_entry_gone());
  expect(chat.use.getState().queueBusy).toBe(false);
  // Com rascunho guardado, é ele que volta.
  created[0].trigger('queue', JSON.stringify({ entries: [{ id: 'a', text: 'oi', ts: 1 }], actions: true }));
  chat.startQueueEdit('a', 'meu rascunho');
  expect(chat.use.getState().queueEditExit).toBeNull();
  await chat.submitQueueEdit('oi mudado');
  expect(chat.use.getState().queueEditExit).toBe('meu rascunho');
});

test('filaCount só conta o Kimi: Claude e Codex usam o painel', () => {
  const state = { events: [ev({ id: 'queued-1', text: 'oi', queued_delivered: false })], pending: [] };
  expect(filaCount(state, 'kimi')).toBe(1);
  expect(filaCount(state, 'claude')).toBe(0);
  expect(filaCount(state, 'codex')).toBe(0);
});

test('onerror fecha e reconecta com backoff crescente', async () => {
  vi.useFakeTimers();
  const registrar = vi.fn();
  configureDiag({ registrar, novoReq: () => 'chat-teste' });
  try {
    historyResponses = [[]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0); // loadHistory + connectSSE

    expect(created).toHaveLength(1);
    created[0].fail(); // erro real: fecha
    expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'sse.retentativa', espera_ms: 3000 }), 'http://10.0.0.1:8765');
    expect(created[0].close).toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(3_000); // primeiro backoff
    expect(created).toHaveLength(2);

    created[1].fail();
    await vi.advanceTimersByTimeAsync(6_000); // backoff dobrou
    expect(created).toHaveLength(3);

    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test.each([
  { type: 'timeout' }, { type: 'close' }, { type: 'error' },
  { type: 'error', xhrStatus: 408 }, { type: 'error', xhrStatus: 429 },
  { type: 'error', xhrStatus: 503 },
])('fonte fechada por $type/$xhrStatus recupera uma vez sem marcar recusa', async (error) => {
  vi.useFakeTimers();
  try {
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);
    const source = created[0];
    source.readyState = 2;
    source.fail(error);
    expect(chat.use.getState().sseRecusado).toBe(false);
    await vi.advanceTimersByTimeAsync(1_000);
    source.fail(error); // repetição atrasada não desloca o timer nem duplica a reconexão.
    await vi.advanceTimersByTimeAsync(2_000);
    expect(created).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(created).toHaveLength(2);
    expect(source.close).toHaveBeenCalledTimes(1);
    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test.each([400, 401, 403, 404, 405, 410])('HTTP %i mantém recusa visível até retry manual', async (xhrStatus) => {
  vi.useFakeTimers();
  try {
    historyResponses = [[ev({ id: 'recusa:1', text: 'conversa preservada' })]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);
    const source = created[0];
    source.fail({ type: 'error', xhrStatus });
    expect(chat.use.getState().sseRecusado).toBe(true);
    expect(chat.use.getState().events[0].text).toBe('conversa preservada');
    await vi.advanceTimersByTimeAsync(60_000);
    expect(created).toHaveLength(1);
    chat.retry();
    expect(chat.use.getState().sseRecusado).toBe(false);
    expect(created).toHaveLength(2);
    source.fail({ type: 'error', xhrStatus });
    expect(chat.use.getState().sseRecusado).toBe(false);
    expect(created[1].close).not.toHaveBeenCalled();
    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test('reabrir a tela depois de uma recusa tenta o stream de novo', async () => {
  vi.useFakeTimers();
  try {
    historyResponses = [[ev({ id: 'reaberta:1' })]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);
    created[0].fail({ type: 'error', xhrStatus: 404 });
    expect(chat.use.getState().sseRecusado).toBe(true);
    chat.release();
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);
    expect(chat.use.getState().sseRecusado).toBe(false);
    expect(created).toHaveLength(2);
    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test('loadOlder abortado por reset não marca failed (B4)', async () => {
  historyResponses = [[ev({ id: 'a:1' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  expect(chat.use.getState().olderFailed).toBe('');
  // forçar o próximo getHistory a rejeitar com AbortError (reset aborta o signal)
  const abortErr = Object.assign(new Error('abort'), { name: 'AbortError' });
  vi.stubGlobal(
    'fetch',
    vi.fn(() => Promise.reject(abortErr)),
  );
  chat.loadOlder();
  // simula o reset que aborta o histAbort enquanto loadOlder está em voo
  created[0].trigger('reset', '{}');
  await tick();
  expect(chat.use.getState().olderFailed).toBe('');
});

test('onopen reseta backoff para 3s (B5)', async () => {
  vi.useFakeTimers();
  try {
    historyResponses = [[]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);
    expect(created).toHaveLength(1);
    created[0].fail();
    await vi.advanceTimersByTimeAsync(3_000);
    expect(created).toHaveLength(2);
    created[1].fail();
    await vi.advanceTimersByTimeAsync(6_000);
    expect(created).toHaveLength(3);
    // conexão 3 abre com sucesso → onopen reseta delay
    created[2].trigger('open', '{}');
    // próxima queda deve usar 3s de novo, não 12s
    created[2].fail();
    await vi.advanceTimersByTimeAsync(3_000);
    expect(created).toHaveLength(4);
    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test('ask_question via SSE abre o stepper', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const payload = { questions: [{ header: 'H', question: 'Q?', multiSelect: false, options: [{ label: 'A', description: 'desc' }] }] };
  created[0].trigger('ask_question', JSON.stringify(payload));
  await tick();
  const s = chat.use.getState();
  expect(s.askOpen).toBe(true);
  expect(s.askPayload?.questions).toHaveLength(1);
});

test('pergunta Codex mantém identidade na reconexão e fecha na resolução', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const payload = { provider: 'codex', request_id: 1, questions: [
    { id: 'choice', header: 'H', question: 'Q?', multiSelect: false, options: [{ label: 'A' }] },
  ] };
  created[0].trigger('ask_question', JSON.stringify(payload));
  const original = chat.use.getState().askPayload;
  chat.closeAsk();
  created[0].trigger('ask_question', JSON.stringify(payload));
  expect(chat.use.getState().askOpen).toBe(false);
  expect(chat.use.getState().askPayload).toBe(original);
  created[0].trigger('ask_question', JSON.stringify({ ...payload, request_id: 2 }));
  expect(chat.use.getState().askOpen).toBe(true);
  expect(chat.use.getState().askPayload?.request_id).toBe(2);
  created[0].trigger('ask_question', 'null');
  expect(chat.use.getState().askOpen).toBe(false);
  expect(chat.use.getState().askPayload).toBeNull();
});

test.each(['codex', undefined])('interaction_id distingue formulários idênticos na reconexão (%s)', async (provider) => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const first = { provider, request_id: 0, interaction_id: 'form-a', questions: [
    { id: 'choice', header: 'H', question: 'Q?', multiSelect: false, options: [{ label: 'A' }] },
  ] };
  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'awaiting_input', interaction_id: 'approval-a', question: 'Permitir?', options: ['Sim'] }));
  created[0].trigger('ask_question', JSON.stringify(first));
  const original = chat.use.getState().askPayload;
  chat.closeAsk();
  created[0].trigger('ask_question', JSON.stringify(first));
  expect(chat.use.getState().askPayload).toBe(original);
  expect(chat.use.getState().askOpen).toBe(false);
  const second = { ...first, interaction_id: 'form-b' };
  created[0].trigger('ask_question', JSON.stringify(second));
  expect(chat.use.getState().askOpen).toBe(true);
  expect(chat.use.getState().askPayload).toEqual(second);
  expect(chat.use.getState().askPayload?.questions).toEqual(original?.questions);
  created[0].trigger('ask_question', 'null');
  expect(chat.use.getState().askOpen).toBe(false);
  expect(chat.use.getState().stateEvent?.interaction_id).toBe('approval-a');
});

test('tokens de prévia seguidos viram um setState por quadro, com o primeiro na hora', async () => {
  vi.useFakeTimers();
  try {
    historyResponses = [[]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0);
    created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
    const vistos: string[] = [];
    const unsub = chat.use.subscribe((s, prev) => { if (s.preview !== prev.preview) vistos.push(s.preview); });
    created[0].trigger('preview', JSON.stringify({ text: 'a' }));
    expect(chat.use.getState().preview).toBe('a');
    created[0].trigger('preview', JSON.stringify({ text: 'ab' }));
    created[0].trigger('preview', JSON.stringify({ text: 'abc' }));
    expect(chat.use.getState().preview).toBe('a');
    await vi.advanceTimersByTimeAsync(50);
    expect(chat.use.getState().preview).toBe('abc');
    expect(vistos).toEqual(['a', 'abc']);
    // O bloco real apaga também o que ainda esperava o quadro.
    created[0].trigger('preview', JSON.stringify({ text: 'abcd' }));
    created[0].trigger('message', JSON.stringify(ev({ id: 'm:9', kind: 'assistant_msg', text: 'fim' })));
    await vi.advanceTimersByTimeAsync(200);
    expect(chat.use.getState().preview).toBe('');
    unsub();
    chat.release();
  } finally {
    vi.useRealTimers();
  }
});

test('preview md flag espelha no store', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  created[0].trigger('preview', JSON.stringify({ text: 'a', md: true }));
  await tick();
  expect(chat.use.getState().previewMd).toBe(true);
  expect(chat.use.getState().preview).toBe('a');
});

test('closeAsk marca askPiDismissed', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();
  const payload = { questions: [{ header: 'H', question: 'Q?', multiSelect: false, options: [{ label: 'A', description: '' }] }] };
  chat.openAsk(payload as never, 't1');
  expect(chat.use.getState().askPiId).toBe('t1');
  expect(chat.use.getState().askOpen).toBe(true);
  chat.closeAsk();
  const s = chat.use.getState();
  expect(s.askOpen).toBe(false);
  expect(s.askPiDismissed).toBe('t1');
  expect(s.askPiId).toBeNull();
});

async function tick(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0));
}

describe('send vai ao servidor da conversa', () => {
  test('sessão de mesmo nome em outra máquina: POST sai para o servidor do store, não para o ativo', async () => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) => new Response('{}', { status: 200 }));
    vi.stubGlobal('fetch', fetchMock);
    await chatStore('srv2', 'sess').send('oi');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('http://10.0.0.2:8765/api/sessions/sess/input');
    expect((init?.headers as Record<string, string>).Authorization).toBe('Bearer tok2');
  });

  test('servidor removido: erro visível e nenhum POST cai no ativo', async () => {
    const fetchMock = vi.fn(async () => new Response('{}', { status: 200 }));
    vi.stubGlobal('fetch', fetchMock);
    const chat = chatStore('sumiu', 'sess');
    await expect(chat.send('oi')).rejects.toThrow();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(chat.use.getState().pending).toHaveLength(0);
  });

  test('rede cai com o POST em voo: erro de envio incerto e nenhuma segunda tentativa', async () => {
    servidores.lista.push({ id: 'srv3', label: 'tres', baseUrl: 'http://10.0.0.3:8765', token: 'tok3' });
    const fetchMock = vi.fn(async () => { throw new TypeError('Network request failed'); });
    vi.stubGlobal('fetch', fetchMock);
    const chat = chatStore('srv3', 'sess');
    const err = await chat.send('oi').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(Error);
    expect(err).not.toBeInstanceOf(TypeError);
    expect((err as Error).message).not.toBe('Network request failed');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(chat.use.getState().pending).toHaveLength(1);
    expect(readDraft('srv3', 'sess')?.submission?.status).toBe('unknown');
  });

  test('recusa HTTP continua como erro com status', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response('{"detail":"x"}', { status: 404 })));
    const err = await chatStore('srv1', 'sess').send('oi').catch((e: unknown) => e);
    expect((err as { status?: number }).status).toBe(404);
  });
});

type Pedido = {
  url: string;
  init?: RequestInit;
  responder: (body: ChatEvent[] | { ok: boolean } | null, opts?: { status?: number; etag?: string }) => void;
};

// fetch que só responde quando o teste manda: permite resposta atrasada chegar fora de ordem.
function fetchManual(): Pedido[] {
  const pedidos: Pedido[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn((url: string, init?: RequestInit) => new Promise<Response>((res) => {
      pedidos.push({
        url: String(url),
        init,
        responder: (body, { status = 200, etag } = {}) => res(new Response(
          body === null ? null : JSON.stringify(body),
          { status, headers: etag ? { ETag: etag } : {} },
        )),
      });
    })),
  );
  return pedidos;
}

test('ACK antigo preserva edição nova, snapshot e eco sobrevivem ao release', async () => {
  const pedidos = fetchManual();
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  pedidos[0].responder([]);
  await tick();
  const first = chat.send('antiga');
  expect(JSON.parse(disk.values.get('draft.v1:srv1::sess')!).submission).toMatchObject({ text: 'antiga', status: 'sending' });
  chat.release();
  expect(chat.use.getState().pending.map((p) => p.text)).toEqual(['antiga']);
  chat.retain();
  pedidos[2].responder([]);
  await tick();
  const current = readDraft('srv1', 'sess')!;
  writeDraft('srv1', 'sess', { ...current, text: 'nova', revision: current.revision + 1 });
  pedidos[1].responder(null);
  await first;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'nova', submission: null });
  expect(chat.use.getState().pending.map((p) => p.text)).toEqual(['antiga']);
  chat.release();
});

test('disco indisponível impede POST e mantém texto; recusa conserva snapshot sem pisar na edição', async () => {
  const chat = chatStore('srv1', 'sess');
  writeDraft('srv1', 'sess', { version: 1, text: 'antiga', revision: 4, transcript: '/t', attachments: [], submission: null });
  const requests = fetchManual();
  disk.fail = true;
  await expect(chat.send('antiga')).rejects.toThrow();
  expect(requests).toHaveLength(0);
  expect(chat.use.getState().pending).toHaveLength(0);
  disk.fail = false;
  const sending = chat.send('antiga');
  const current = readDraft('srv1', 'sess')!;
  writeDraft('srv1', 'sess', { ...current, text: 'nova', revision: 5 });
  const rejected = expect(sending).rejects.toMatchObject({ status: 404 });
  requests[0].responder(null, { status: 404 });
  await rejected;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'nova', revision: 5,
    submission: { text: 'antiga', draftRevision: 4, status: 'rejected' } });
  expect(chat.use.getState().pending).toHaveLength(0);
});

test('reabertura só lê; reenvio explícito faz um POST e recusa toque concorrente', async () => {
  writeDraft('srv1', 'sess', { version: 1, text: 'oi', revision: 1, transcript: '/t', attachments: [],
    submission: { text: 'oi', draftRevision: 1, status: 'sending' } });
  historyResponses = [[ev({ id: 'old:1', text: 'oi' })]];
  const chat = chatStore('srv1', 'sess');
  chat.retain(); await tick();
  expect(historyCalls).toBe(1);
  expect(readDraft('srv1', 'sess')?.submission?.status).toBe('unknown');
  chat.use.setState({ pending: [{ id: 'pending-old', text: 'oi' }] });
  const requests = fetchManual();
  const sending = chat.send('oi');
  expect(requests).toHaveLength(1);
  expect(isSubmitting('srv1', 'sess')).toBe(true);
  await expect(chat.send('oi')).rejects.toThrow();
  expect(requests).toHaveLength(1);
  expect(chat.use.getState().pending.map((p) => p.text)).toEqual(['oi']);
  requests[0].responder(null);
  await sending;
  expect(isSubmitting('srv1', 'sess')).toBe(false);
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: '', submission: null });
  chat.release();
});

test('snapshot incerto com outra edição exige recuperação sem fazer POST', async () => {
  writeDraft('srv1', 'sess', { version: 1, text: 'outro', revision: 2, transcript: '/t', attachments: [],
    submission: { text: 'oi', draftRevision: 1, status: 'unknown' } });
  const requests = fetchManual();
  await expect(chatStore('srv1', 'sess').send('outro')).rejects.toThrow(m.composer_submission_recover_first());
  expect(requests).toHaveLength(0);
  expect(isSubmitting('srv1', 'sess')).toBe(false);
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'outro', submission: { text: 'oi', status: 'unknown' } });
});

test('falha parcial de broadcast conserva detalhe por destino e snapshot incerto', async () => {
  await expect(submitConversationDraft('srv1', 'sess', 'oi', async () => {
    throw new Error('chegou em a; não chegou em b');
  })).rejects.toThrow('chegou em a; não chegou em b');
  expect(readDraft('srv1', 'sess')?.submission?.status).toBe('unknown');
  expect(isSubmitting('srv1', 'sess')).toBe(false);
});

test('envio esvazia o campo na hora; ACK só tira o snapshot e conserva o texto digitado de novo', async () => {
  writeDraft('srv1', 'sess', { version: 1, text: 'oi', revision: 1, transcript: null, attachments: [], submission: null });
  const requests = fetchManual();
  const sending = chatStore('srv1', 'sess').send('oi');
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: '', revision: 2, submission: { text: 'oi', draftRevision: 1 } });
  writeDraft('srv1', 'sess', { ...readDraft('srv1', 'sess')!, text: 'oi', transcript: '/first', revision: 3 });
  requests[0].responder(null);
  await sending;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'oi', revision: 3, transcript: '/first', submission: null });
});

test('recusa devolve texto e áudios ao campo vazio; com texto novo conserva o novo e o snapshot', async () => {
  const recording = { id: 'voice-1', serverPath: '/uploaded/voice.m4a' };
  writeDraft('srv1', 'sess', { version: 1, text: 'ditado', revision: 7, transcript: '/first',
    recordings: [recording], attachments: [], submission: null });
  const requests = fetchManual();
  const sending = expect(chatStore('srv1', 'sess').send('ditado', 7)).rejects.toMatchObject({ status: 400 });
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: '', recordings: [] });
  requests[0].responder(null, { status: 400 });
  await sending;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'ditado', recordings: [recording],
    submission: { text: 'ditado', status: 'rejected' } });

  writeDraft('srv1', 'sess', { version: 1, text: 'outro', revision: 9, transcript: '/first', attachments: [], submission: null });
  const later = expect(chatStore('srv1', 'sess').send('outro', 9)).rejects.toMatchObject({ status: 400 });
  writeDraft('srv1', 'sess', { ...readDraft('srv1', 'sess')!, text: 'digitado depois', revision: 11 });
  requests[1].responder(null, { status: 400 });
  await later;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'digitado depois', revision: 11,
    submission: { text: 'outro', status: 'rejected' } });
});

test('recusa com texto novo devolve os áudios enviados sem duplicar os novos', async () => {
  const sent = { id: 'voice-1', serverPath: '/uploaded/voice.m4a' };
  const added = { id: 'voice-2', serverPath: '/uploaded/new.m4a' };
  writeDraft('srv1', 'sess', { version: 1, text: 'ditado', revision: 7, transcript: '/first',
    recordings: [sent], attachments: [], submission: null });
  const requests = fetchManual();
  const sending = expect(chatStore('srv1', 'sess').send('ditado', 7)).rejects.toMatchObject({ status: 400 });
  writeDraft('srv1', 'sess', { ...readDraft('srv1', 'sess')!, text: 'novo', revision: 9, recordings: [added] });
  requests[0].responder(null, { status: 400 });
  await sending;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'novo', recordings: [sent, added] });
});

test('recusa não devolve o texto ao campo que foi editado e apagado durante o envio', async () => {
  writeDraft('srv1', 'sess', { version: 1, text: 'antigo', revision: 7, transcript: '/first', attachments: [], submission: null });
  const requests = fetchManual();
  const sending = expect(chatStore('srv1', 'sess').send('antigo', 7)).rejects.toMatchObject({ status: 400 });
  writeDraft('srv1', 'sess', { ...readDraft('srv1', 'sess')!, text: '', revision: 10 });
  requests[0].responder(null, { status: 400 });
  await sending;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: '', revision: 10,
    submission: { text: 'antigo', status: 'rejected' } });
});

test('envio retira já os chips de áudio enviados e conserva os ditados depois', async () => {
  const recording = { id: 'voice-1', serverPath: '/uploaded/voice.m4a' };
  writeDraft('srv1', 'sess', { version: 1, text: 'ditado', revision: 7, transcript: '/first',
    recordings: [recording], attachments: [], submission: null });
  const requests = fetchManual();
  const sending = chatStore('srv1', 'sess').send('ditado', 7);
  expect(readDraft('srv1', 'sess')?.recordings).toEqual([]);
  const later = { id: 'voice-2', serverPath: '/uploaded/new.m4a' };
  writeDraft('srv1', 'sess', { ...readDraft('srv1', 'sess')!, text: 'novo', revision: 9, recordings: [later] });
  requests[0].responder(null);
  await sending;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'novo', recordings: [later], submission: null });
});

test.each([null, 400, 503])('citação sem rascunho conserva edição nova durante envio com status %s', async (status) => {
  const text = 'Mensagem com citação';
  const requests = fetchManual();
  const sending = chatStore('srv1', 'sess').send(text, 8);
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: '', revision: 9,
    submission: { text, draftRevision: 8 } });
  writeDraft('srv1', 'sess', { ...readDraft('srv1', 'sess')!, text: 'Próxima mensagem', revision: 10 });
  const result = status === null ? sending : expect(sending).rejects.toMatchObject({ status });
  requests[0].responder(null, status === null ? undefined : { status });
  await result;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'Próxima mensagem', revision: 10,
    submission: status === null ? null : { text, draftRevision: 8, status: status === 400 ? 'rejected' : 'unknown' } });
});

test('ACK limpa campo da mesma revisão e conserva demais metadados do rascunho', async () => {
  writeDraft('srv1', 'sess', { version: 1, text: 'legenda', revision: 7, transcript: '/first',
    attachments: [{ uri: 'file:///own/image', name: 'image.jpg', mime: 'image/jpeg', kind: 'image' }], submission: null });
  const requests = fetchManual();
  const sending = chatStore('srv1', 'sess').send('legenda — 📎 imagem: /uploaded/image.jpg', 7);
  requests[0].responder(null);
  await sending;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: '', revision: 8, transcript: '/first', submission: null,
    attachments: [{ uri: 'file:///own/image', name: 'image.jpg' }] });
});

test('history recuperado reconcilia eco preservado sem duplicar user_msg', async () => {
  const chat = chatStore('srv1', 'sess');
  historyResponses = [[]];
  chat.retain(); await tick();
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{}', { status: 200 })));
  await chat.send('oi');
  chat.release();
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify([ev({ id: 'real:1', text: 'oi' })]), { status: 200 })));
  chat.retain(); await tick();
  expect(chat.use.getState().pending).toEqual([]);
  expect(chat.use.getState().events.map((e) => e.id)).toEqual(['real:1']);
  chat.release();
});

describe('primeiro plano', () => {
  test.each([
    { live: ev({ id: 'live', text: 'Ao vivo' }), tail: [ev({ id: 'rest' })], ids: ['rest', 'live'] },
    { live: ev({ id: 'old', text: 'Versão ao vivo' }), tail: [ev({ id: 'rest' })], ids: ['rest', 'old'] },
    { live: ev({ id: 'live', text: 'Ao vivo' }),
      tail: [ev({ id: 'real', text: 'Mesmo texto' }), ev({ id: 'queued-new', text: 'Mesmo texto' })],
      ids: ['real', 'queued-new', 'live'] },
  ])('retry sem costura conserva os eventos recebidos durante a consulta: $ids', async ({ live, tail, ids }) => {
    const requests = fetchManual();
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    requests[0].responder([ev({ id: 'old', text: 'Conversa anterior' })]);
    await tick();
    const source = created[0];
    chat.retry();
    expect(source.close).not.toHaveBeenCalled();
    source.trigger('message', JSON.stringify(live));
    requests[1].responder(tail);
    await tick();
    expect(chat.use.getState().events.map((e) => e.id)).toEqual(ids);
    expect(chat.use.getState().events.at(-1)?.text).toBe(live.text);
    chat.release();
  });

  test('fundo fecha stream e timers sem perder conversa; volta sincroniza uma vez sem duplicar', async () => {
    const pedidos = fetchManual();
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    pedidos[0].responder([ev({ id: 'a:1' }), ev({ id: 'a:2', kind: 'assistant_msg', text: 'olá' })]);
    await tick();
    created[0].trigger('message', JSON.stringify(ev({ id: 'a:3', kind: 'assistant_msg', text: 'três' })), 'a:3');
    const pergunta = { questions: [{ header: 'H', question: 'Q?', multiSelect: false, options: [{ label: 'A' }] }] };
    created[0].trigger('ask_question', JSON.stringify(pergunta));
    created[0].fail(); // backoff agendado: o fundo precisa desarmá-lo

    chat.setForeground(false);
    expect(created[0].close).toHaveBeenCalled();
    await new Promise((r) => setTimeout(r, 3_100));
    expect(created).toHaveLength(1);
    expect(pedidos).toHaveLength(1);
    const s = chat.use.getState();
    expect(s.events.map((e) => e.id)).toEqual(['a:1', 'a:2', 'a:3']);
    expect(s.askOpen).toBe(true);

    chat.setForeground(true);
    chat.setForeground(true); // volta repetida não abre segunda leitura
    expect(pedidos).toHaveLength(2);
    expect(pedidos[1].url).toContain('10.0.0.1:8765/api/sessions/sess/history?limit=400');
    pedidos[1].responder([ev({ id: 'a:2', kind: 'assistant_msg', text: 'olá' }),
      ev({ id: 'a:3', kind: 'assistant_msg', text: 'três' }), ev({ id: 'a:4', kind: 'assistant_msg', text: 'quatro' })],
    { etag: '"v1"' });
    await tick();
    expect(chat.use.getState().events.map((e) => e.id)).toEqual(['a:1', 'a:2', 'a:3', 'a:4']);
    expect(chat.use.getState().askOpen).toBe(true);
    expect(created).toHaveLength(2);
    expect(created[1].url).toContain('last_event_id=a%3A3');

    // A próxima volta pergunta pelo ETag guardado; 304 não mexe na lista.
    chat.setForeground(false);
    chat.setForeground(true);
    expect(new Headers(pedidos[2].init?.headers).get('If-None-Match')).toBe('"v1"');
    pedidos[2].responder(null, { status: 304 });
    await tick();
    expect(chat.use.getState().events.map((e) => e.id)).toEqual(['a:1', 'a:2', 'a:3', 'a:4']);
    expect(created).toHaveLength(3);
    chat.release();
  });

  test('consulta antiga não sobrescreve a geração atual', async () => {
    const pedidos = fetchManual();
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    pedidos[0].responder([ev({ id: 'b:1' }), ev({ id: 'b:2' })]);
    await tick();
    chat.loadOlder();
    chat.setForeground(false);
    chat.setForeground(true);
    chat.setForeground(false);
    chat.setForeground(true);
    expect(pedidos).toHaveLength(4);
    pedidos[3].responder([ev({ id: 'b:2' }), ev({ id: 'b:3' })]);
    await tick();
    // Respostas velhas chegando depois (sem costura trocaria a lista inteira).
    pedidos[2].responder([ev({ id: 'velho:9' })]);
    pedidos[1].responder([ev({ id: 'velho:0' }), ev({ id: 'b:1' })]);
    await tick();
    const s = chat.use.getState();
    expect(s.events.map((e) => e.id)).toEqual(['b:1', 'b:2', 'b:3']);
    expect(s.olderFailed).toBe('');
    expect(created).toHaveLength(2);
    chat.release();
  });

  test('cauda sem costura limpa o cursor; reentrada conserva o cursor da conversa preservada', async () => {
    const pedidos = fetchManual();
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    pedidos[0].responder([ev({ id: 'x:1' })]);
    await tick();
    created[0].trigger('message', JSON.stringify(ev({ id: 'x:1' })), 'x:1');
    chat.setForeground(false);
    chat.setForeground(true);
    pedidos[1].responder([ev({ id: 'y:9' })]);
    await tick();
    expect(chat.use.getState().events.map((e) => e.id)).toEqual(['y:9']);
    expect(created.at(-1)!.url).not.toContain('last_event_id');

    created.at(-1)!.trigger('message', JSON.stringify(ev({ id: 'y:9' })), 'y:9');
    chat.release();
    chat.retain();
    pedidos[2].responder([ev({ id: 'y:9' })]);
    await tick();
    expect(chat.use.getState().events.map((e) => e.id)).toEqual(['y:9']);
    expect(created.at(-1)!.url).toContain('last_event_id=y%3A9');
    chat.release();
  });

  test('store novo respeita o fundo e só conecta com consumidor na volta', async () => {
    historyResponses = [[ev({ id: 'c:1' })]];
    setChatsForeground(false);
    const semConsumidor = chatStore('srv1', 'livre');
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await tick();
    expect(historyCalls).toBe(0);
    expect(created).toHaveLength(0);

    setChatsForeground(true);
    await tick();
    expect(historyCalls).toBe(1);
    expect(created).toHaveLength(1);
    expect(created[0].url).toContain('/api/sessions/sess/events');
    expect(chat.use.getState().events.map((e) => e.id)).toEqual(['c:1']);
    expect(semConsumidor.use.getState().loading).toBe(true);
    chat.release();
  });

  test('primeira carga cortada pelo fundo recarrega inteira na volta', async () => {
    const pedidos = fetchManual();
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    chat.setForeground(false);
    pedidos[0].responder([ev({ id: 'tarde:1' })]);
    await tick();
    expect(chat.use.getState().events).toEqual([]);
    chat.setForeground(true);
    expect(pedidos[1].url).toContain('/history?limit=400');
    expect(new Headers(pedidos[1].init?.headers).get('If-None-Match')).toBeNull();
    pedidos[1].responder([ev({ id: 'd:1' })]);
    await tick();
    expect(chat.use.getState().events.map((e) => e.id)).toEqual(['d:1']);
    expect(chat.use.getState().loading).toBe(false);
    chat.release();
  });

  test('envio em voo sobrevive ao fundo e segue ao servidor da conversa', async () => {
    const pedidos = fetchManual();
    const chat = chatStore('srv2', 'sess');
    chat.retain();
    expect(pedidos[0].url).toContain('10.0.0.2:8765/api/sessions/sess/history');
    pedidos[0].responder([]);
    await tick();
    expect(created[0].url).toContain('10.0.0.2:8765/api/sessions/sess/events');

    const envio = chat.send('oi');
    chat.setForeground(false);
    const post = pedidos[1];
    expect(post.url).toContain('10.0.0.2:8765/api/sessions/sess/input');
    expect(post.init?.signal?.aborted ?? false).toBe(false);
    expect(chat.use.getState().pending.map((p) => p.text)).toEqual(['oi']);
    post.responder([]);
    await envio;
    expect(chat.use.getState().pending.map((p) => p.text)).toEqual(['oi']);
    chat.release();
  });
});


describe('envio Direto e Jev pelo mesmo rascunho', () => {
  test('Claude com terminal manda direto só o texto novo e mantém o recibo', async () => {
    const chat = chatStore('srv1', 'fila/sess');
    vi.mocked(fetch).mockResolvedValue(new Response(JSON.stringify({ ok: true, delivered: false, steered: true, native: false })));
    await chat.send('usar azul', undefined, { kind: 'direct', terminalClaude: true });
    const [url, init] = vi.mocked(fetch).mock.calls[0];
    expect(url).toBe('http://10.0.0.1:8765/api/sessions/fila%2Fsess/input');
    expect(JSON.parse(init!.body as string)).toEqual({ text: 'usar azul', steer: true });
    expect(readDraft('srv1', 'fila/sess')?.submission).toBeNull();
  });

  test('Jev envia contexto e mostra aviso da decisão sem reenviar', async () => {
    const chat = chatStore('srv1', 'sess');
    vi.mocked(fetch).mockResolvedValue(new Response(JSON.stringify({ ok: true, delivered: false, steered: false, native: false, timing_note: 'work_changed' })));
    await chat.send('usar azul', undefined, { kind: 'jev', task: 'criar tela', message: 'usar azul' });
    expect(JSON.parse(vi.mocked(fetch).mock.calls[0][1]!.body as string)).toEqual({ text: 'usar azul', jev_task: 'criar tela', jev_message: 'usar azul' });
    expect(chat.use.getState().deliveryFeedback).toEqual({ text: m.native_queue_jev_work_changed(), warning: true });
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  test('aviso da entrega some sozinho e não apaga o aviso mais novo', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    try {
      const chat = chatStore('srv1', 'sess');
      vi.mocked(fetch).mockResolvedValue(new Response(JSON.stringify({ ok: true, timing_note: 'queue' })));
      await chat.send('usar azul', undefined, { kind: 'jev', task: 'criar tela', message: 'usar azul' });
      expect(chat.use.getState().deliveryFeedback).toEqual({ text: m.native_queue_jev_queued(), warning: false });
      vi.advanceTimersByTime(8_000);
      chat.showDeliveryFeedback({ text: 'mais novo', warning: true });
      vi.advanceTimersByTime(8_000);
      expect(chat.use.getState().deliveryFeedback).toEqual({ text: 'mais novo', warning: true });
      vi.advanceTimersByTime(4_000);
      expect(chat.use.getState().deliveryFeedback).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  test('queda no envio Jev conserva rascunho e eco incertos sem segundo POST', async () => {
    const chat = chatStore('srv1', 'sess');
    vi.mocked(fetch).mockRejectedValue(new TypeError('network'));
    await expect(chat.send('usar azul', undefined, { kind: 'jev', task: 'criar tela', message: 'usar azul' })).rejects.toThrow(m.chat_envio_incerto());
    expect(readDraft('srv1', 'sess')?.submission?.status).toBe('unknown');
    expect(chat.use.getState().pending.map((p) => p.text)).toEqual(['usar azul']);
    expect(chat.use.getState().deliveryFeedback).toBeNull();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  test('recusa explícita do Jev preserva o campo e remove só o eco recusado', async () => {
    const chat = chatStore('srv1', 'sess');
    vi.mocked(fetch).mockResolvedValue(new Response('{"detail":"runtime_unhealthy"}', { status: 422 }));
    await expect(chat.send('usar azul', undefined, { kind: 'jev', task: 'criar tela', message: 'usar azul' })).rejects.toMatchObject({ status: 422 });
    expect(readDraft('srv1', 'sess')?.submission?.status).toBe('rejected');
    expect(readDraft('srv1', 'sess')?.text).toBe('usar azul');
    expect(chat.use.getState().pending).toEqual([]);
  });
});

describe('envio incerto que a conversa confirma', () => {
  function stubInput(input: () => Promise<Response>) {
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (String(url).endsWith('/input')) return input();
      const body = historyResponses[Math.min(historyCalls, historyResponses.length - 1)] ?? [];
      historyCalls++;
      return new Response(JSON.stringify(body), { status: 200 });
    }));
  }
  const agora = () => Date.now() / 1000;

  test('mensagem que aparece na conversa depois da queda apaga o aviso e o texto devolvido', async () => {
    historyResponses = [[]];
    stubInput(async () => { throw new TypeError('Network request failed'); });
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await tick();
    await expect(chat.send('como está?')).rejects.toThrow(m.chat_envio_incerto());
    expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'como está?', submission: { status: 'unknown' } });
    created[0].trigger('message', JSON.stringify(ev({ id: 'u:1', text: 'como está?', ts: agora() })));
    expect(readDraft('srv1', 'sess')).toMatchObject({ text: '', submission: null });
  });

  test('mensagem igual de antes do envio não confirma', async () => {
    historyResponses = [[]];
    stubInput(async () => { throw new TypeError('Network request failed'); });
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await tick();
    await expect(chat.send('ok')).rejects.toThrow(m.chat_envio_incerto());
    created[0].trigger('message', JSON.stringify(ev({ id: 'u:velha', text: 'ok', ts: agora() - 3600 })));
    expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'ok', submission: { status: 'unknown' } });
  });

  test('queda com o erro nativo do Expo mostra a frase do app, não a mensagem crua', async () => {
    historyResponses = [[]];
    stubInput(async () => { throw new Error('fetch failed: UnexpectedException: A conexão de rede foi perdida.'); });
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await tick();
    await expect(chat.send('A', undefined, { kind: 'direct', terminalClaude: true })).rejects.toThrow(m.chat_envio_incerto());
    expect(readDraft('srv1', 'sess')).toMatchObject({ submission: { status: 'unknown' } });
  });

  test('"sim" igual que já estava na conversa não confirma o "sim" que caiu', async () => {
    historyResponses = [[ev({ id: 'u:antes', text: 'sim', ts: agora() - 10 })]];
    stubInput(async () => { throw new TypeError('Network request failed'); });
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await tick();
    await expect(chat.send('sim')).rejects.toThrow(m.chat_envio_incerto());
    created[0].trigger('message', JSON.stringify(ev({ id: 'u:antes', text: 'sim', ts: agora() - 10 })));
    expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'sim', submission: { status: 'unknown' } });
  });

  test('uma linha de outra mensagem não confirma o envio', async () => {
    historyResponses = [[]];
    stubInput(async () => { throw new TypeError('Network request failed'); });
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await tick();
    await expect(chat.send('ok')).rejects.toThrow(m.chat_envio_incerto());
    created[0].trigger('message', JSON.stringify(ev({ id: 'u:2', text: 'ok\ncontinua o plano', ts: agora() })));
    expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'ok', submission: { status: 'unknown' } });
  });

  test('mensagem que chega com o POST em voo: a queda depois não vira erro', async () => {
    historyResponses = [[]];
    let fail: ((error: Error) => void) | undefined;
    stubInput(() => new Promise((_, reject) => { fail = reject; }));
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await tick();
    const sending = chat.send('como está?');
    await vi.waitFor(() => expect(fail).toBeDefined());
    created[0].trigger('message', JSON.stringify(ev({ id: 'u:1', text: 'como está?', ts: agora() })));
    fail!(new TypeError('Network request failed'));
    await expect(sending).resolves.toBeUndefined();
    expect(readDraft('srv1', 'sess')).toMatchObject({ text: '', submission: null });
  });
});
