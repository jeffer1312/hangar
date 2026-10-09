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
  expect(filaCount(chat.use.getState(), 'kimi')).toBe(1);
  // Claude com terminal também conta: o chip manda a fila da TUI com ctrl+x ctrl+s, como no PWA.
  expect(filaCount(chat.use.getState(), 'claude')).toBe(1);
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
  responder: (body: ChatEvent[] | null, opts?: { status?: number; etag?: string }) => void;
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
  writeDraft('srv1', 'sess', { version: 1, text: 'antiga', revision: 4, transcript: '/t', attachment: null, submission: null });
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
  writeDraft('srv1', 'sess', { version: 1, text: 'oi', revision: 1, transcript: '/t', attachment: null,
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
  writeDraft('srv1', 'sess', { version: 1, text: 'outro', revision: 2, transcript: '/t', attachment: null,
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

test('ACK compara revisão mesmo quando a nova edição tem texto idêntico; associação inicial conserva identidade', async () => {
  writeDraft('srv1', 'sess', { version: 1, text: 'oi', revision: 1, transcript: null, attachment: null, submission: null });
  const requests = fetchManual();
  const sending = chatStore('srv1', 'sess').send('oi');
  const current = readDraft('srv1', 'sess')!;
  writeDraft('srv1', 'sess', { ...current, transcript: '/first', revision: 2 });
  requests[0].responder(null);
  await sending;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: 'oi', revision: 2, transcript: '/first', submission: null });
});

test('ACK limpa campo da mesma revisão e conserva demais metadados do rascunho', async () => {
  writeDraft('srv1', 'sess', { version: 1, text: 'legenda', revision: 7, transcript: '/first',
    attachment: { uri: 'file:///own/image', name: 'image.jpg', mime: 'image/jpeg', kind: 'image' }, submission: null });
  const requests = fetchManual();
  const sending = chatStore('srv1', 'sess').send('legenda — 📎 imagem: /uploaded/image.jpg', 7);
  requests[0].responder(null);
  await sending;
  expect(readDraft('srv1', 'sess')).toMatchObject({ text: '', revision: 8, transcript: '/first', submission: null,
    attachment: { uri: 'file:///own/image', name: 'image.jpg' } });
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
