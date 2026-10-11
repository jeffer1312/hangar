import { beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import type { LiveVoiceHandlers } from './liveVoice';

const fakes = vi.hoisted(() => ({
  calls: [] as { handlers: LiveVoiceHandlers; start: Mock; setScreen: Mock }[],
  home: { id: 'casa', label: 'Casa', baseUrl: 'https://casa.ts.net', token: 'a' },
  vps: { id: 'pwa-vps', label: 'VPS', baseUrl: 'https://vps.ts.net', token: 'b' },
}));
const { home, vps } = fakes;

vi.mock('./liveVoice', () => ({
  LiveVoiceCall: class {
    start = vi.fn(async () => {});
    setScreen = vi.fn();
    setMuted = vi.fn();
    setMode = vi.fn();
    stop = vi.fn();
    constructor(_audio: unknown, public handlers: LiveVoiceHandlers) { fakes.calls.push(this); }
  },
}));
vi.mock('./peers', () => ({ listarPeers: vi.fn(async () => [{ id: 'vps', base_url: 'https://vps.ts.net:8765', token: '••' }]) }));
vi.mock('./auth', () => ({ listServers: () => [fakes.home, fakes.vps] }));
// Cada teste reimporta o store (resetModules): o core real recompilado a cada vez estoura o prazo do teste.
vi.mock('@hangar/core', () => ({ baseOf: (s: { baseUrl: string }) => s.baseUrl, probeServerResponse: vi.fn() }));
// Cada texto vira o nome da própria chave: dá para ver qual recusa saiu.
const voiceKey = (key: string | symbol): key is string => typeof key === 'string' && key.startsWith('live_voice_');
vi.mock('../paraglide/messages', () => new Proxy({}, {
  get: (_t, key) => (voiceKey(key) ? () => key : undefined),
  has: (_t, key) => voiceKey(key),
}));

let onVisibility: () => void;
const doc = { visibilityState: 'hidden' };

async function store() {
  vi.resetModules();
  fakes.calls.length = 0;
  vi.stubGlobal('document', Object.assign(doc, {
    visibilityState: 'hidden',
    createElement: () => ({}),
    body: { appendChild: vi.fn() },
    addEventListener: (_type: string, fn: () => void) => { onVisibility = fn; },
  }));
  return (await import('./liveVoiceStore.svelte')).liveVoiceStore;
}

function visible() {
  doc.visibilityState = 'visible';
  onVisibility();
}

const settle = () => new Promise(resolve => setTimeout(resolve, 0));

beforeEach(() => vi.unstubAllGlobals());

describe('liveVoiceStore', () => {
  it('maps_screen_and_switch_through_peers_by_host', async () => {
    const voice = await store();
    voice.setScreen({ server: vps, name: 'web' });
    await voice.start(home);
    const call = fakes.calls[0];
    // A chamada não espera os peers; a tela de outra máquina segue quando eles chegam.
    expect(call.start).toHaveBeenCalledWith(home, null);
    await settle();
    expect(call.setScreen).toHaveBeenLastCalledWith({ server: 'vps', name: 'web' });

    voice.setScreen({ server: home, name: 'hangar' });
    expect(call.setScreen).toHaveBeenLastCalledWith({ server: '', name: 'hangar' });

    const navigate = vi.fn(() => true);
    voice.registerNavigator(navigate);
    expect(call.handlers.switchTo({ server: 'vps', baseUrl: 'https://vps.ts.net:8765', name: 'api' })).toBe(true);
    expect(navigate).toHaveBeenLastCalledWith({ server: vps, name: 'api' });
    expect(call.handlers.switchTo({ server: '', baseUrl: null, name: 'hangar' })).toBe(true);
    expect(navigate).toHaveBeenLastCalledWith({ server: home, name: 'hangar' });
    expect(call.handlers.switchTo({ server: 'other', baseUrl: null, name: 'x' })).toBe(false);
  });

  it('screen_actions_run_where_the_screen_registered_them', async () => {
    const voice = await store();
    await voice.start(home);
    const { handlers } = fakes.calls[0];
    expect(handlers.actions().map(a => a.id)).toContain('git');
    expect(handlers.runAction('git', null)).toBe('live_voice_action_unavailable');
    const git = vi.fn((): true | string => true);
    const off = voice.registerActions({ git });
    expect(handlers.runAction('git', null)).toBe(true);
    expect(git).toHaveBeenCalledOnce();
    // Uma tela que sai não derruba a ação que outra registrou depois.
    const other = vi.fn((): true | string => true);
    voice.registerActions({ git: other });
    off();
    expect(handlers.runAction('git', null)).toBe(true);
    expect(other).toHaveBeenCalledOnce();
    expect(handlers.runAction('voice-panel', null)).toBe(true);
    expect(voice.open).toBe(true);
    expect(handlers.runAction('voice-panel-close', null)).toBe(true);
    expect(voice.open).toBe(false);
    expect(handlers.runAction('nada', null)).toBe('live_voice_action_unknown');
  });

  it('reopens_once_on_visible_after_connection_lost', async () => {
    const voice = await store();
    await voice.start(home);
    const call = fakes.calls[0];

    call.handlers.failed('connection_lost', undefined);
    visible();
    await vi.waitFor(() => expect(call.start).toHaveBeenCalledTimes(2));
    visible();
    await settle();
    expect(call.start).toHaveBeenCalledTimes(2);

    call.handlers.failed('connection_lost', undefined);
    call.handlers.taken();
    visible();
    await settle();
    expect(call.start).toHaveBeenCalledTimes(2);

    call.handlers.failed('connection_lost', undefined);
    voice.stop();
    visible();
    await settle();
    expect(call.start).toHaveBeenCalledTimes(2);
  });
});
