import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { configureApi } from '@hangar/core';
import type { ChatEvent } from '@hangar/core';
import { chatStore, _resetChatsForTests, filaCount } from './chat';

// EventSource falso injetado via configureApi (mesmo padrão de sessions.test.ts)
type FakeES = {
  url: string;
  listeners: Record<string, ((e: { data: string; lastEventId?: string }) => void)[]>;
  close: ReturnType<typeof vi.fn>;
  trigger: (type: string, data: string, lastEventId?: string) => void;
  fail: () => void;
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
    fail() {
      this.onerror?.(new Error('tcp'));
    },
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
      return fake.onerror;
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
    readyState: 1,
  };
}

// fetch falso pro /history — pilha de respostas por ordem de chamada
let historyResponses: ChatEvent[][] = [];
let historyCalls = 0;

function ev(partial: Partial<ChatEvent> & { id: string }): ChatEvent {
  return { kind: 'user_msg', text: partial.id, ts: 1_700_000_000, ...partial } as ChatEvent;
}

beforeEach(() => {
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

test('(c) preview some quando o assistant_msg real chega', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();

  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
  created[0].trigger('preview', JSON.stringify({ text: 'pensando…', md: true, full: true }));
  expect(chat.use.getState().preview).toBe('pensando…');

  created[0].trigger('preview', JSON.stringify({ text: 'pensando mais', md: true, full: true }));
  expect(chat.use.getState().preview).toBe('pensando mais');

  created[0].trigger(
    'message',
    JSON.stringify(ev({ id: 'm:1', kind: 'assistant_msg', text: 'resposta final' })),
  );
  expect(chat.use.getState().preview).toBe('');
  expect(chat.use.getState().events).toHaveLength(1);
});

test('(c2) preview vazio durante working NÃO apaga a bolha; sair de working apaga', async () => {
  historyResponses = [[]];
  const chat = chatStore('srv1', 'sess');
  chat.retain();
  await tick();

  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'working' }));
  created[0].trigger('preview', JSON.stringify({ text: 'rascunho' }));
  expect(chat.use.getState().preview).toBe('rascunho');

  // entre ferramentas o extrator manda "" — bolha fica
  created[0].trigger('preview', JSON.stringify({ text: '' }));
  expect(chat.use.getState().preview).toBe('rascunho');

  created[0].trigger('state', JSON.stringify({ session: 'sess', state: 'idle' }));
  expect(chat.use.getState().preview).toBe('');
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

test('(f) filaCount conta pending + queued-* e zera quando o real chega', async () => {
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
  expect(filaCount(chat.use.getState())).toBe(1);
  // sintético queued-* chega com mesmo texto -> pending reconciliado, queued entra
  // restaura fetch falso de history pra não quebrar o SSE trigger, mas mantém send mock
  // o trigger não depende de fetch, só do store
  created[0].trigger('message', JSON.stringify(ev({ id: 'queued-1', text: 'oi tudo bem' })));
  await tick();
  expect(chat.use.getState().pending).toHaveLength(0);
  expect(filaCount(chat.use.getState())).toBe(1);
  // render queued translúcido: events contém queued-*
  expect(chat.use.getState().events.some((e) => e.id.startsWith('queued-'))).toBe(true);
  // real chega -> queued sai, fila zera
  created[0].trigger('message', JSON.stringify(ev({ id: 'a:10', text: 'oi tudo bem' })));
  await tick();
  expect(filaCount(chat.use.getState())).toBe(0);
  expect(chat.use.getState().events.some((e) => e.id.startsWith('queued-'))).toBe(false);
  await p;
  vi.stubGlobal('fetch', origFetch as never);
});

test('onerror fecha e reconecta com backoff crescente', async () => {
  vi.useFakeTimers();
  try {
    historyResponses = [[]];
    const chat = chatStore('srv1', 'sess');
    chat.retain();
    await vi.advanceTimersByTimeAsync(0); // loadHistory + connectSSE

    expect(created).toHaveLength(1);
    created[0].fail(); // erro real: fecha
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

async function tick(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0));
}
