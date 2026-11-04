import type { ChatEvent, EventSourceLike, SessionInfo, StateEvent } from '@hangar/core';
import data from './demoData.json';

// A revisão do Google Play não tem servidor próprio: este endereço responde dentro do app, sem rede.
export const DEMO_BASE_URL = 'http://127.0.0.1:1';
export const DEMO_TOKEN = 'demo';

type Listener = (ev: { data: string; lastEventId?: string }) => void;

const now = () => Math.floor(Date.now() / 1000);
let seq = 0;
const nextId = () => `demo-${++seq}`;

// shortcut: conteúdo só em inglês, o idioma da revisão; traduzir se a demonstração virar recurso para usuários.
const SESSIONS = data.sessions.map(({ ago, reply, ...session }) => ({
  ...session, state: 'idle', headless: true, tracked: true,
  last_activity: now() - ago, last_reply: reply, last_reply_at: now() - ago,
} as SessionInfo));

const message = (kind: 'user_msg' | 'assistant_msg', text: string, ago: number): ChatEvent =>
  ({ kind, id: nextId(), text, ts: now() - ago });

const CHATS: Record<string, ChatEvent[]> = Object.fromEntries(Object.entries(data.chats).map(([session, events]) =>
  [session, events.map(({ ago, ...event }) => ({ ...event, id: nextId(), ts: now() - ago } as ChatEvent))]));

class DemoEventSource implements EventSourceLike {
  readyState = 1;
  onerror: ((ev: unknown) => void) | null = null;
  onopen: ((ev: unknown) => void) | null = null;
  private listeners = new Map<string, Set<Listener>>();
  private ping = setInterval(() => this.emit('ping', {}), 10_000);

  constructor(readonly session: string | null) {
    sources.add(this);
    // Quem abriu só registra os ouvintes depois do retorno.
    setTimeout(() => {
      if (this.readyState === 2) return;
      this.onopen?.({ type: 'open' });
      if (session === null) this.emit('sessions', SESSIONS);
      else this.emit('state', stateOf(session, 'idle'));
    }, 0);
  }

  addEventListener(type: string, fn: Listener) {
    const set = this.listeners.get(type) ?? new Set<Listener>();
    this.listeners.set(type, set.add(fn));
  }

  removeEventListener(type: string, fn: Listener) {
    this.listeners.get(type)?.delete(fn);
  }

  close() {
    this.readyState = 2;
    clearInterval(this.ping);
    sources.delete(this);
  }

  emit(type: string, payload: unknown) {
    const data = JSON.stringify(payload);
    this.listeners.get(type)?.forEach((fn) => fn({ data }));
  }
}

const sources = new Set<DemoEventSource>();

const stateOf = (session: string, state: StateEvent['state']): StateEvent => ({ session, state, headless: true });

function publish(session: string, type: string, payload: unknown) {
  sources.forEach((source) => { if (source.session === session) source.emit(type, payload); });
}

function record(session: string, event: ChatEvent) {
  CHATS[session].push(event);
  publish(session, 'message', event);
}

function answer(session: string, text: string) {
  record(session, message('user_msg', text, 0));
  publish(session, 'state', stateOf(session, 'working'));
  setTimeout(() => {
    record(session, message('assistant_msg', data.reply, 0));
    publish(session, 'state', stateOf(session, 'idle'));
  }, 800);
}

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

const unavailable = () => json({ detail: data.unavailable }, 404);

const VOICE_MODEL = { model: null, effort: 'low', tier: null };
const VOICE_SETTINGS = {
  enabled: false, codex: false, jev: false, voices: [], call: { active: false, client: null },
  settings: { voice: null, codex_account: 'demo', organizer: { direct: VOICE_MODEL, plan: VOICE_MODEL } },
};

function route(path: string, init?: RequestInit): Response {
  if (path === '/api/sessions') return !init?.method || init.method === 'GET' ? json(SESSIONS) : unavailable();
  if (path === '/api/config') return json({ campos: {}, somente_leitura: {} });
  if (path === '/api/peers/identificador') return json({ identificador: 'demo' });
  // Sem Codex o botão de voz some; um 404 aqui viraria "Falha" vermelho no topo da lista.
  if (path === '/api/voice/settings') return json(VOICE_SETTINGS);
  const [, name, action] = path.match(/^\/api\/sessions\/([^/]+)\/(history|input)$/) ?? [];
  const session = name && decodeURIComponent(name);
  if (!session || !CHATS[session]) return unavailable();
  if (action === 'history') return json(CHATS[session]);
  answer(session, (JSON.parse(String(init?.body)) as { text: string }).text);
  return json({ ok: true });
}

const pathOf = (url: string) => url.slice(DEMO_BASE_URL.length).split('?')[0];

export const isDemoUrl = (url: string) => url.startsWith(`${DEMO_BASE_URL}/`);

export function installDemoFetch(): void {
  const real = globalThis.fetch.bind(globalThis);
  globalThis.fetch = (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    return isDemoUrl(url) ? Promise.resolve(route(pathOf(url), init)) : real(input, init);
  };
}

export function openDemoEventSource(url: string): EventSourceLike {
  const [, name] = pathOf(url).match(/^\/api\/sessions\/([^/]+)\/events$/) ?? [];
  return new DemoEventSource(name && name !== 'events' ? decodeURIComponent(name) : null);
}
