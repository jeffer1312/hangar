import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { LiveVoiceRequestError } from '@hangar/core';
import type { LiveVoiceHandlers, LiveVoiceSettingsReply, LiveVoiceState, Server, VoiceMemory } from '@hangar/core';
import * as m from '../../paraglide/messages';

const api = vi.hoisted(() => ({ settings: vi.fn(), save: vi.fn(), memory: vi.fn(), forget: vi.fn(), accounts: vi.fn(), models: vi.fn(), peers: vi.fn(), diag: vi.fn() }));
vi.mock('@hangar/core', async importOriginal => ({
  ...await importOriginal<typeof import('@hangar/core')>(),
  baseOf: (server: Server) => server.baseUrl,
  loadLiveVoiceSettings: api.settings, saveLiveVoiceSettings: api.save,
  listVoiceMemory: api.memory, forgetVoiceMemory: api.forget,
  listarCredenciais: api.accounts, modelOptionsForServer: api.models, probeServerResponse: api.peers, registrarDiag: api.diag,
}));
vi.mock('../../stores/servers', async () => {
  const { create } = await import('zustand');
  return { useServers: create(() => ({ servers: [] as Server[], activeId: null as string | null })) };
});

const native = vi.hoisted(() => {
  class Stream {
    track = { enabled: true, onended: null, stop: vi.fn() };
    getTracks = () => [this.track];
    getAudioTracks = () => [this.track];
    release = vi.fn();
  }
  class Peer {
    static all: Peer[] = [];
    connectionState = 'new';
    onconnectionstatechange: (() => void) | null = null;
    addTrack = vi.fn();
    createDataChannel = () => ({ close: vi.fn() });
    createOffer = async () => ({ type: 'offer', sdp: 'offer' });
    setLocalDescription = async () => {};
    close = vi.fn();
    constructor() { Peer.all.push(this); }
    connect() { this.connectionState = 'connected'; this.onconnectionstatechange?.(); }
  }
  return { Peer, Stream, start: vi.fn(), stop: vi.fn(), media: vi.fn(), microphonePermission: vi.fn(), androidPermission: vi.fn(),
    platform: { OS: 'ios', Version: 18 }, listeners: new Set<(event: { reason: string }) => void>() };
});
vi.mock('react-native', () => ({ Platform: native.platform,
  PermissionsAndroid: { PERMISSIONS: { POST_NOTIFICATIONS: 'android.permission.POST_NOTIFICATIONS', BLUETOOTH_CONNECT: 'android.permission.BLUETOOTH_CONNECT' },
    RESULTS: { GRANTED: 'granted' }, request: native.androidPermission },
}));
vi.mock('react-native-webrtc', () => ({ RTCPeerConnection: native.Peer, mediaDevices: { getUserMedia: native.media },
  permissions: { request: native.microphonePermission } }));
vi.mock('../../../modules/live-voice-audio', () => ({ default: {
  start: native.start, stop: native.stop,
  addListener: (name: string, listener: (event: { reason: string }) => void) => {
    if (name === 'interruption') native.listeners.add(listener);
    return { remove: () => { native.listeners.delete(listener); } };
  },
} }));
class NativeSocket {
  static OPEN = 1;
  static all: NativeSocket[] = [];
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  messages: Record<string, unknown>[] = [];
  send = (text: string) => { this.messages.push(JSON.parse(text)); };
  close = vi.fn();
  constructor() { NativeSocket.all.push(this); }
  open() { this.readyState = 1; this.onopen?.(); }
  receive(message: object) { this.onmessage?.({ data: JSON.stringify(message) }); }
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const settle = () => new Promise<void>(resolve => setImmediate(resolve));
const a: Server = { id: 'saved-a', label: 'machine-a', baseUrl: 'https://same.example/a', token: 'owner-a', lan: { url: 'http://192.168.1.2:8765', id: 'machine-a' } };
const b: Server = { id: 'saved-b', label: 'machine-b', baseUrl: 'https://same.example/b', token: 'owner-b', lan: { url: 'http://192.168.1.2:8766', id: 'machine-b' } };
const reply = (account = 'account-a'): LiveVoiceSettingsReply => ({ enabled: true, codex: true, jev: true, voices: ['sol'], call: { active: false, client: null },
  settings: { voice: 'sol', codex_account: account, style: 'conversational', organizer: { direct: { model: 'direct-model', effort: 'medium', tier: null }, plan: { model: 'plan-model', effort: 'high', tier: null } } } });
const memory: VoiceMemory = { id: 'one', kind: 'preferencia', text: 'text', learned: '2026-10-15', confirmed: '2026-10-15' };
const state = (count = 0): LiveVoiceState => ({ phase: 'live', mode: 'direct', activity: 'idle', draft: null, error: null, plan: null, effective: null, context: null,
  limits: { five_hour: null, seven_day: null }, backstage: [], thought: '', action: null, followed: [], server: 'machine-a', client: 'mobile', memory: count });

type Instance = { handlers: LiveVoiceHandlers; start: ReturnType<typeof vi.fn>; stop: ReturnType<typeof vi.fn>; setScreen: ReturnType<typeof vi.fn>; setMuted: ReturnType<typeof vi.fn>; setMode: ReturnType<typeof vi.fn> };
let useLiveVoice: typeof import('./liveVoiceStore').useLiveVoice;
let servers: typeof import('../../stores/servers').useServers;
let audio: typeof import('./audioSession');
let calls: Instance[];
let importGate: Promise<void>;
let startGate: Promise<void>;
let realTransport: typeof import('./liveVoice') | null = null;

beforeEach(async () => {
  vi.resetModules();
  for (const mock of Object.values(api)) mock.mockReset();
  api.settings.mockResolvedValue(reply());
  api.save.mockImplementation(async (_server, settings) => settings);
  api.memory.mockResolvedValue([memory]);
  api.forget.mockResolvedValue(undefined);
  api.accounts.mockResolvedValue([]);
  api.models.mockResolvedValue({ kind: 'codex', reduced: false, models: [] });
  api.peers.mockImplementation(async () => ({ ok: true, json: async () => [{ id: 'peer-b-on-a', base_url: b.baseUrl, web_url: b.lan!.url, token: 'masked' }] }));
  calls = [];
  realTransport = null;
  native.start.mockReset().mockResolvedValue(undefined);
  native.stop.mockReset().mockResolvedValue(undefined);
  native.media.mockReset().mockImplementation(async () => new native.Stream());
  native.microphonePermission.mockReset().mockResolvedValue(true);
  native.androidPermission.mockReset().mockResolvedValue('granted');
  native.platform.OS = 'ios'; native.platform.Version = 18;
  native.listeners.clear(); native.Peer.all = []; NativeSocket.all = [];
  importGate = Promise.resolve();
  startGate = Promise.resolve();
  vi.doMock('./liveVoice', async () => {
    await importGate;
    if (realTransport) return realTransport;
    return { LiveVoiceCall: class {
      handlers: LiveVoiceHandlers;
      start = vi.fn(async () => { await startGate; });
      stop = vi.fn(async () => {});
      setScreen = vi.fn();
      setMuted = vi.fn();
      setMode = vi.fn();
      constructor(handlers: LiveVoiceHandlers) { this.handlers = handlers; calls.push(this); }
    } };
  });
  servers = (await import('../../stores/servers')).useServers;
  servers.setState({ servers: [a, b], activeId: b.id });
  audio = await import('./audioSession');
  ({ useLiveVoice } = await import('./liveVoiceStore'));
});

function openA() {
  useLiveVoice.getState().setRoute({ serverId: a.id, name: 'session-a', kind: 'chat' });
  useLiveVoice.getState().openFor(a.id, 'session-a');
}
async function startA() { openA(); await useLiveVoice.getState().start(); return calls[0]; }

describe('uma chamada capturada, sem depender do servidor ativo', () => {
  test('captura A com B ativo; abrir pela conversa B e fechar folha conserva a chamada A', async () => {
    const instance = await startA();
    expect(instance.start).toHaveBeenCalledWith(a, { server: '', name: 'session-a' }, expect.any(Number));
    expect(api.settings).toHaveBeenCalledWith(a);
    expect(api.peers).toHaveBeenCalledWith(a, '/api/peers');
    useLiveVoice.getState().dismiss();
    useLiveVoice.getState().openFor(b.id, 'session-b');
    expect(useLiveVoice.getState().server).toEqual(a);
    expect(useLiveVoice.getState().view.serverLabel).toBe('machine-a');
    expect(instance.stop).not.toHaveBeenCalled();
    expect(calls).toHaveLength(1);
    expect(servers.getState().activeId).toBe(b.id);
  });

  test('dois toques têm uma trava síncrona; parar durante import não abre áudio depois', async () => {
    const gate = deferred<void>();
    importGate = gate.promise;
    openA();
    const first = useLiveVoice.getState().start();
    const second = useLiveVoice.getState().start();
    expect(useLiveVoice.getState().view.phase).toBe('connecting');
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(true);
    await settle();
    await useLiveVoice.getState().stop();
    gate.resolve();
    await Promise.all([first, second]);
    expect(calls).toHaveLength(0);
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(false);
    expect(useLiveVoice.getState().view.phase).toBe('closed');
    await useLiveVoice.getState().start();
    expect(calls).toHaveLength(1);
  });

  test('parar durante start pendente cancela a instância; resposta tardia não altera a seguinte', async () => {
    const gate = deferred<void>();
    startGate = gate.promise;
    openA();
    const pending = useLiveVoice.getState().start(); await settle();
    const old = calls[0];
    await useLiveVoice.getState().stop();
    expect(old.stop).toHaveBeenCalledOnce();
    startGate = Promise.resolve();
    await useLiveVoice.getState().start();
    calls[1].handlers.phase('live');
    gate.resolve(); await pending;
    old.handlers.phase('closed');
    expect(useLiveVoice.getState().view.phase).toBe('live');
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(true);
  });

  test('servidor que some durante import libera connecting mesmo antes da reconciliação do Root', async () => {
    const gate = deferred<void>();
    importGate = gate.promise;
    openA();
    const pending = useLiveVoice.getState().start();
    await settle();
    servers.setState({ servers: [b] });
    gate.resolve(); await pending;
    expect(calls).toHaveLength(0);
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(false);
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'closed', error: { code: 'server_removed', detail: m.chat_servidor_removido() } });
  });

  test('callback antigo não altera outra tentativa; erro fatal sobrevive ao closed', async () => {
    const old = await startA();
    old.handlers.failed('microphone', 'denied');
    old.handlers.phase('closed');
    expect(useLiveVoice.getState().view.error).toEqual({ code: 'microphone', detail: 'denied' });
    await useLiveVoice.getState().retry();
    const current = calls[1];
    current.handlers.phase('live');
    old.handlers.failed('connection_lost');
    old.handlers.taken();
    old.handlers.state(state(9));
    old.handlers.levels({ input: 1, output: 1 });
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'live', error: null, state: null, levels: { input: 0, output: 0 } });
  });

  test('taken libera apenas o local, sem stop nem retomada automática', async () => {
    const instance = await startA();
    instance.handlers.taken();
    instance.handlers.phase('closed');
    await settle();
    expect(instance.stop).not.toHaveBeenCalled();
    expect(calls).toHaveLength(1);
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(false);
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'closed', error: { code: 'taken' } });
    useLiveVoice.getState().setForeground(false);
    useLiveVoice.getState().setForeground(true);
    expect(calls).toHaveLength(1);
  });

  test('fechar aviso após taken permite escolher B sem retomar A automaticamente', async () => {
    const instance = await startA();
    instance.handlers.taken();
    useLiveVoice.getState().dismiss();
    expect(useLiveVoice.getState().view.error).toBeNull();
    useLiveVoice.getState().openFor(b.id, 'session-b');
    expect(useLiveVoice.getState().server).toEqual(b);
    expect(calls).toHaveLength(1);
    expect(instance.stop).not.toHaveBeenCalled();
  });

  test('servidor removido encerra sem usar B; convite não começa e ainda alcança chamada ativa', async () => {
    const instance = await startA();
    servers.setState({ servers: [b] });
    useLiveVoice.getState().reconcileServers();
    await settle();
    expect(instance.stop).toHaveBeenCalledOnce();
    expect(useLiveVoice.getState().view.error).toEqual({ code: 'server_removed', detail: m.chat_servidor_removido() });
    expect(useLiveVoice.getState().server?.id).toBe(a.id);
    await useLiveVoice.getState().stop();
    servers.setState({ servers: [{ ...b, invite: true }] });
    useLiveVoice.getState().openFor(b.id, 'guest-session');
    await useLiveVoice.getState().start();
    expect(calls).toHaveLength(1);
  });
});

describe('seleção explícita do servidor da voz', () => {
  test('após erro em A seleciona B, limpa estado e memória e liga B sem mudar a seleção global', async () => {
    servers.setState({ activeId: a.id });
    const instance = await startA();
    instance.handlers.state(state(5));
    await useLiveVoice.getState().refreshMemory();
    instance.handlers.failed('connection_lost', 'old-machine');
    useLiveVoice.getState().selectServer(b.id);
    expect(useLiveVoice.getState().server).toEqual(b);
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'idle', serverLabel: 'machine-b', state: null, error: null,
      memory: { loading: false, entries: [], error: null, forgetError: null } });
    expect(api.settings).toHaveBeenLastCalledWith(b);
    await useLiveVoice.getState().start();
    expect(calls[1].start).toHaveBeenCalledWith(b, null, expect.any(Number));
    expect(servers.getState().activeId).toBe(a.id);
  });

  test.each(['connecting', 'live'] as const)('seleção não muda destino enquanto %s', async phase => {
    openA();
    useLiveVoice.getState().selectServer(b.id);
    expect(useLiveVoice.getState().server?.id).toBe(b.id);
    await useLiveVoice.getState().start();
    calls[0].handlers.phase(phase);
    const requests = api.settings.mock.calls.length;
    useLiveVoice.getState().selectServer(a.id);
    expect(useLiveVoice.getState().server?.id).toBe(b.id);
    expect(api.settings).toHaveBeenCalledTimes(requests);
    expect(calls[0].stop).not.toHaveBeenCalled();
    await useLiveVoice.getState().stop();
  });

  test.each(['connecting', 'live'] as const)('servidor invalidado em %s conserva erro até escolha explícita de B', async phase => {
    const instance = await startA();
    instance.handlers.phase(phase);
    servers.setState({ servers: [{ ...a, removed: true }, b] });
    useLiveVoice.getState().reconcileServers();
    await settle();
    expect(instance.stop).toHaveBeenCalledOnce();
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'closed', error: { code: 'server_removed' } });
    useLiveVoice.getState().reconcileServers();
    expect(useLiveVoice.getState().view.error?.code).toBe('server_removed');
    expect(useLiveVoice.getState().server?.id).toBe(a.id);
    useLiveVoice.getState().selectServer(b.id);
    expect(useLiveVoice.getState().server?.id).toBe(b.id);
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'idle', error: null, state: null, memory: { entries: [] } });
    expect(api.settings).toHaveBeenLastCalledWith(b);
    await useLiveVoice.getState().start();
    expect(calls[1].start).toHaveBeenCalledWith(b, null, expect.any(Number));
    await useLiveVoice.getState().stop();
  });

  test('respostas atrasadas de A não preenchem ajustes nem memória de B', async () => {
    const oldSettings = deferred<LiveVoiceSettingsReply>();
    const oldMemory = deferred<VoiceMemory[]>();
    api.settings.mockReturnValueOnce(oldSettings.promise).mockResolvedValueOnce(reply('b-account'));
    api.memory.mockReturnValueOnce(oldMemory.promise);
    openA();
    const reading = useLiveVoice.getState().refreshMemory();
    useLiveVoice.getState().selectServer(b.id);
    expect(useLiveVoice.getState().server?.id).toBe(b.id);
    oldSettings.resolve(reply('old-a-account')); oldMemory.resolve([memory]);
    await reading; await settle();
    expect(useLiveVoice.getState().view.settings.reply?.settings.codex_account).toBe('b-account');
    expect(useLiveVoice.getState().view.memory.entries).toEqual([]);
  });
});

describe('escolha da voz preservada e somente máquinas habilitadas', () => {
  test('fechar e reabrir por A conserva a escolha explícita B sem persistência nem troca global', () => {
    openA();
    useLiveVoice.getState().selectServer(b.id);
    useLiveVoice.getState().dismiss();
    useLiveVoice.getState().openFor(a.id, 'session-a');
    expect(useLiveVoice.getState().server?.id).toBe(b.id);
    expect(servers.getState().activeId).toBe(b.id);
  });

  test.each([
    { disabled: true }, { removed: true }, { invite: true }, { inviteEnded: true }, { token: '' },
  ])('seleção recusa máquina inelegível %j', patch => {
    openA();
    useLiveVoice.getState().selectServer(b.id);
    servers.setState({ servers: [{ ...a, ...patch }, b] });
    const requests = api.settings.mock.calls.length;
    useLiveVoice.getState().selectServer(a.id);
    expect(useLiveVoice.getState().server?.id).toBe(b.id);
    expect(api.settings).toHaveBeenCalledTimes(requests);
  });

  test.each([{ disabled: true }, { removed: true }])('escolha inválida %j recua para ativo elegível e depois primeira própria', patch => {
    openA();
    useLiveVoice.getState().selectServer(b.id);
    useLiveVoice.getState().dismiss();
    servers.setState({ servers: [a, { ...b, ...patch }], activeId: a.id });
    useLiveVoice.getState().reconcileServers();
    expect(useLiveVoice.getState().server?.id).toBe(a.id);
    expect(useLiveVoice.getState().open).toBe(false);
    expect(api.settings).toHaveBeenLastCalledWith(a);
    servers.setState({ servers: [{ ...a, disabled: true }, b], activeId: a.id });
    useLiveVoice.getState().reconcileServers();
    expect(useLiveVoice.getState().server?.id).toBe(b.id);
    servers.setState({ servers: [{ ...b, disabled: true }] });
    useLiveVoice.getState().reconcileServers();
    expect(useLiveVoice.getState().server).toBeNull();
    expect(useLiveVoice.getState().view.serverLabel).toBe('');
  });
});

describe('restauração com o transporte real e prazo desde o store', () => {
  beforeEach(async () => {
    realTransport = await vi.importActual<typeof import('./liveVoice')>('./liveVoice');
    vi.useFakeTimers(); vi.setSystemTime(0); vi.stubGlobal('WebSocket', NativeSocket);
  });
  afterEach(async () => {
    await useLiveVoice.getState().stop();
    vi.useRealTimers(); vi.unstubAllGlobals();
  });
  async function live() {
    openA(); await useLiveVoice.getState().start();
    const socket = NativeSocket.all.at(-1)!; socket.open(); native.Peer.all.at(-1)!.connect();
    expect(useLiveVoice.getState().view.phase).toBe('live');
    return socket;
  }

  test.each(['android', 'ios'])('%s: permissões demoradas e volta ao app não usam deadline antigo do store', async os => {
    native.platform.OS = os; native.platform.Version = os === 'android' ? 33 : 18;
    const stages = os === 'android' ? ['notification', 'bluetooth', 'microphone'] : ['microphone'];
    const waiting = new Map(stages.map(stage => [stage, { entered: deferred<void>(), grant: deferred<void>() }]));
    native.androidPermission.mockImplementation(async (permission: string) => {
      const gate = waiting.get(permission === 'android.permission.POST_NOTIFICATIONS' ? 'notification' : 'bluetooth')!;
      gate.entered.resolve(); await gate.grant.promise; return 'granted';
    });
    native.microphonePermission.mockImplementation(async () => {
      const gate = waiting.get('microphone')!;
      gate.entered.resolve(); await gate.grant.promise; return true;
    });
    openA(); const starting = useLiveVoice.getState().start();
    for (const gate of waiting.values()) {
      await gate.entered.promise;
      await vi.advanceTimersByTimeAsync(90000);
      useLiveVoice.getState().setForeground(false);
      useLiveVoice.getState().setForeground(true);
      expect(useLiveVoice.getState().view).toMatchObject({ phase: 'connecting', error: null });
      expect(audio.useVoiceAudioState.getState().voiceActive).toBe(true);
      expect(native.media).not.toHaveBeenCalled();
      expect(native.start).not.toHaveBeenCalled();
      expect(NativeSocket.all).toHaveLength(0);
      gate.grant.resolve();
    }
    await starting;
    useLiveVoice.getState().setForeground(false);
    useLiveVoice.getState().setForeground(true);
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'connecting', error: null });
    expect(native.microphonePermission).toHaveBeenCalledExactlyOnceWith({ name: 'microphone' });
    expect(native.media).toHaveBeenCalledOnce();
    const socket = NativeSocket.all[0]; socket.open(); native.Peer.all[0].connect();
    await vi.advanceTimersByTimeAsync(1000);
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'live', error: null });
    expect(socket.close).not.toHaveBeenCalled();
  });

  test.each(['sheet', 'notification'] as const)('Encerrar por %s publica audio_restore do Native.stop real sem stop duplicado', async source => {
    const socket = await live();
    native.stop.mockRejectedValueOnce(new Error('private-restore-failure'));
    if (source === 'sheet') await useLiveVoice.getState().stop();
    else {
      for (const listener of native.listeners) listener({ reason: 'stop' });
      await vi.advanceTimersByTimeAsync(0);
    }
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'closed', error: { code: 'audio_restore' } });
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(false);
    expect(native.stop).toHaveBeenCalledOnce();
    expect(socket.messages.filter(message => message.type === 'stop')).toHaveLength(1);
    expect(api.diag).toHaveBeenCalledWith({ evento: 'voice.cleanup_failed', nivel: 'erro', codigo: 'audio_restore' });
    expect(JSON.stringify(api.diag.mock.calls)).not.toContain('private-restore-failure');
  });

  test.each(['taken', 'fatal'] as const)('restauração que falha preserva %s e não manda stop remoto', async terminal => {
    const socket = await live();
    native.stop.mockRejectedValueOnce(new Error('restore-failure'));
    socket.receive(terminal === 'taken' ? { type: 'taken' } : { type: 'error', code: 'account_missing', detail: 'chosen' });
    await vi.advanceTimersByTimeAsync(0);
    expect(useLiveVoice.getState().view.error).toEqual(terminal === 'taken' ? { code: 'taken' } : { code: 'account_missing', detail: 'chosen' });
    expect(socket.messages.some(message => message.type === 'stop')).toBe(false);
    expect(api.diag).toHaveBeenCalledWith({ evento: 'voice.cleanup_failed', nivel: 'erro', codigo: 'audio_restore' });
  });

  test('rejeição de limpeza e callback antigos não fecham a nova tentativa', async () => {
    const old = await live();
    const cleanup = deferred<void>(), entered = deferred<void>();
    const oldMessage = old.onmessage!;
    native.stop.mockImplementationOnce(() => { entered.resolve(); return cleanup.promise; });
    const stopping = useLiveVoice.getState().stop(); await entered.promise;
    const starting = useLiveVoice.getState().start();
    cleanup.reject(new Error('old-restore'));
    await Promise.all([stopping, starting]);
    const current = NativeSocket.all.at(-1)!; current.open(); native.Peer.all.at(-1)!.connect();
    oldMessage({ data: JSON.stringify({ type: 'error', code: 'audio_restore' }) });
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'live', error: null });
    expect(current.close).not.toHaveBeenCalled();
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(true);
  });

  test('limpeza anterior pendente vence em 60s desde start do store sem esperar stop', async () => {
    await live();
    const cleanup = deferred<void>(), entered = deferred<void>();
    native.stop.mockImplementationOnce(() => { entered.resolve(); return cleanup.promise; });
    const stopping = useLiveVoice.getState().stop(); await entered.promise;
    const starting = useLiveVoice.getState().start();
    await vi.advanceTimersByTimeAsync(60000); await starting;
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'closed', error: { code: 'opening_timeout' } });
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(false);
    expect(native.media).toHaveBeenCalledOnce();
    cleanup.resolve(); await stopping;
    expect(native.media).toHaveBeenCalledOnce();
    expect(NativeSocket.all).toHaveLength(1);
  });

  test('importação pendente expira e retorno tardio não abre transporte', async () => {
    const gate = deferred<void>(); importGate = gate.promise;
    openA(); const starting = useLiveVoice.getState().start();
    await vi.advanceTimersByTimeAsync(60000); await starting;
    expect(useLiveVoice.getState().view).toMatchObject({ phase: 'closed', error: { code: 'opening_timeout' } });
    gate.resolve(); await vi.dynamicImportSettled();
    expect(native.media).not.toHaveBeenCalled();
    expect(NativeSocket.all).toHaveLength(0);
  });
});

describe('navegação e ferramentas somente sobre a tela visível', () => {
  test('usa ids dos peers de A, URL completa e navega só para credencial já salva', async () => {
    const navigate = vi.fn();
    useLiveVoice.getState().registerNavigation(navigate);
    const instance = await startA();
    await settle();
    expect(instance.handlers.switchTo({ server: 'peer-b-on-a', baseUrl: b.baseUrl + '/', name: 'space / name' })).toBe(true);
    expect(navigate).toHaveBeenLastCalledWith({ kind: 'session', serverId: b.id, name: 'space / name' });
    // Navegar não antecipa o fato: somente a rota confirmada publica B.
    expect(instance.setScreen).not.toHaveBeenCalledWith({ server: 'peer-b-on-a', name: 'space / name' });
    useLiveVoice.getState().setRoute({ serverId: b.id, name: 'space / name', kind: 'chat' });
    expect(instance.setScreen).toHaveBeenLastCalledWith({ server: 'peer-b-on-a', name: 'space / name' });
    expect(useLiveVoice.getState().server?.id).toBe(a.id);
    expect(instance.handlers.switchTo({ server: '', baseUrl: a.baseUrl, name: 'self' })).toBe(true);
    expect(navigate).toHaveBeenLastCalledWith({ kind: 'session', serverId: a.id, name: 'self' });
    expect(instance.handlers.switchTo({ server: b.id, baseUrl: b.baseUrl, name: 'bad-local-id' })).toBe(false);
    expect(instance.handlers.switchTo({ server: 'peer-b-on-a', baseUrl: 'https://same.example/c', name: 'unknown-url' })).toBe(false);
    servers.setState({ servers: [a] });
    expect(instance.handlers.switchTo({ server: 'peer-b-on-a', baseUrl: b.baseUrl, name: 'no-local-token' })).toBe(false);
  });

  test('falha do mapa de peers registra só código e mantém recusa segura', async () => {
    api.peers.mockRejectedValueOnce(new Error('network-token-must-not-leak'));
    useLiveVoice.getState().registerNavigation(vi.fn());
    const instance = await startA(); await settle();
    expect(instance.handlers.switchTo({ server: 'peer-b-on-a', baseUrl: b.baseUrl, name: 'not-mapped' })).toBe(false);
    expect(api.diag).toHaveBeenCalledWith({ evento: 'voz.peers_falhou', nivel: 'aviso', codigo: 'connection_lost' });
    expect(JSON.stringify(api.diag.mock.calls)).not.toContain('network-token-must-not-leak');
  });

  test('mesmo host em portas e caminhos distintos não colide; URL duplicada é recusada', async () => {
    const navigate = vi.fn();
    useLiveVoice.getState().registerNavigation(navigate);
    const instance = await startA();
    await settle();
    expect(instance.handlers.switchTo({ server: 'peer-b-on-a', baseUrl: b.lan!.url, name: 'port-b' })).toBe(true);
    servers.setState({ servers: [a, b, { ...b, id: 'duplicate-b', token: 'another-token' }] });
    expect(instance.handlers.switchTo({ server: 'peer-b-on-a', baseUrl: b.baseUrl, name: 'ambiguous' })).toBe(false);
    expect(instance.handlers.switchTo({ server: '', baseUrl: 'https://same.example', name: 'host-only' })).toBe(false);
    expect(servers.getState().servers).toHaveLength(3);
  });

  test('segundo plano conserva áudio, recusa ações/troca e republica tela ao voltar', async () => {
    const navigate = vi.fn();
    useLiveVoice.getState().registerNavigation(navigate);
    const instance = await startA();
    instance.handlers.phase('live');
    useLiveVoice.getState().setForeground(false);
    expect(instance.handlers.switchTo({ server: '', baseUrl: null, name: 'hidden' })).toBe(false);
    expect(instance.handlers.runAction('settings', null)).not.toBe(true);
    expect(instance.handlers.runAction('voice-panel', null)).not.toBe(true);
    expect(instance.stop).not.toHaveBeenCalled();
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(true);
    useLiveVoice.getState().setRoute({ serverId: b.id, name: 'background-route', kind: 'chat' });
    useLiveVoice.getState().setForeground(true);
    expect(instance.setScreen).toHaveBeenLastCalledWith({ server: 'peer-b-on-a', name: 'background-route' });
    expect(navigate).not.toHaveBeenCalled();
  });

  test('catálogo não inventa telas; terminal exige chat vivo e cleanup não apaga registro novo', async () => {
    const navigate = vi.fn();
    const oldNavigation = useLiveVoice.getState().registerNavigation(vi.fn());
    const currentNavigation = useLiveVoice.getState().registerNavigation(navigate);
    oldNavigation();
    const oldChat = useLiveVoice.getState().registerChat({ serverId: a.id, name: 'session-a', terminal: false });
    const currentChat = useLiveVoice.getState().registerChat({ serverId: a.id, name: 'session-a', terminal: true });
    oldChat();
    const instance = await startA();
    expect(instance.handlers.actions().map(action => action.id)).toEqual(['session-list', 'settings', 'voice-panel', 'voice-panel-close', 'activity', 'terminal']);
    expect(instance.handlers.runAction('git', null)).not.toBe(true);
    expect(instance.handlers.runAction('settings', '../unsafe')).not.toBe(true);
    expect(instance.handlers.runAction('settings', 'voice')).toBe(true);
    expect(navigate).toHaveBeenLastCalledWith({ kind: 'settings', serverId: a.id, section: 'voice' });
    expect(instance.handlers.runAction('terminal', null)).toBe(true);
    currentChat();
    expect(instance.handlers.runAction('terminal', null)).not.toBe(true);
    useLiveVoice.getState().setRoute(null);
    expect(instance.handlers.runAction('activity', null)).not.toBe(true);
    currentNavigation();
    expect(instance.handlers.switchTo({ server: '', baseUrl: null, name: 'unmounted' })).toBe(false);
  });
});

describe('coexistência com ditado', () => {
  test('recusa início antes do áudio/import quando ditado já está preparando', async () => {
    const release = audio.beginVoiceDictation();
    openA();
    await useLiveVoice.getState().start();
    expect(useLiveVoice.getState().view.error?.code).toBe('dictation_active');
    expect(calls).toHaveLength(0);
    expect(audio.useVoiceAudioState.getState().voiceActive).toBe(false);
    release();
  });
  test.each([false, true])('hold e volta preservam preferência muted=%s', async (muted) => {
    const instance = await startA();
    if (muted) useLiveVoice.getState().toggleMute();
    const release = audio.beginVoiceDictation();
    const releaseOther = audio.beginVoiceDictation();
    expect(useLiveVoice.getState().view).toMatchObject({ dictating: true, muted: true });
    useLiveVoice.getState().toggleMute();
    release(); release();
    expect(useLiveVoice.getState().view.dictating).toBe(true);
    releaseOther();
    expect(useLiveVoice.getState().view).toMatchObject({ dictating: false, muted });
    expect(instance.setMuted).toHaveBeenLastCalledWith(muted);
  });
});

describe('configuração e memória do servidor capturado', () => {
  test('falha de rede ao carregar ajustes mostra mensagem traduzida sem detalhe do transporte', async () => {
    const transportError = 'fetch failed: Fetch request has been canceled';
    api.settings.mockRejectedValueOnce(new Error(transportError));
    openA();
    await useLiveVoice.getState().loadSettings(a);
    expect(useLiveVoice.getState().view.settings.error).toBe(m.native_network_error());
    expect(useLiveVoice.getState().view.settings.error).not.toContain(transportError);
  });

  test('falha HTTP com código preserva mensagem e detalhe ao carregar ajustes', async () => {
    api.settings.mockRejectedValueOnce(new LiveVoiceRequestError('account_missing', 'chosen-account', 409));
    openA();
    await useLiveVoice.getState().loadSettings(a);
    expect(useLiveVoice.getState().view.settings).toMatchObject({ error: 'account_missing: chosen-account', loading: false, reply: null });
  });

  test('resposta antiga de settings não vence retry; erro aparece e retry recupera', async () => {
    const old = deferred<LiveVoiceSettingsReply>();
    api.settings.mockReturnValueOnce(old.promise).mockRejectedValueOnce(new LiveVoiceRequestError('settings_failed')).mockResolvedValueOnce(reply('new-account'));
    openA();
    await useLiveVoice.getState().retry();
    expect(useLiveVoice.getState().view.settings).toMatchObject({ error: 'settings_failed', loading: false, reply: null });
    await useLiveVoice.getState().retry();
    old.resolve(reply('old-account'));
    await settle();
    expect(useLiveVoice.getState().view.settings.reply?.settings.codex_account).toBe('new-account');
  });

  test('recarregar ajustes lê settings atuais e expõe só contas Codex', async () => {
    openA(); await settle();
    api.settings.mockResolvedValueOnce(reply('changed-on-another-device'));
    api.accounts.mockResolvedValueOnce([{ id: 'claude', tipo: 'claude' }, { id: 'codex', tipo: 'codex' }]);
    useLiveVoice.getState().openSettings(); await settle();
    expect(api.settings).toHaveBeenLastCalledWith(a);
    expect(useLiveVoice.getState().view.settings.reply?.settings.codex_account).toBe('changed-on-another-device');
    expect(useLiveVoice.getState().view.settings.accounts.map(account => account.id)).toEqual(['codex']);
    expect(api.models).toHaveBeenLastCalledWith(a, 'codex', null, null, 'changed-on-another-device');
  });

  test('falha HTTP sem código preserva causa ao carregar modelos', async () => {
    api.models.mockRejectedValueOnce(Object.assign(new Error('502: erro_codex_model_list: catálogo indisponível'), { status: 502 }));
    openA();
    await useLiveVoice.getState().loadSettings(a);
    useLiveVoice.getState().openSettings(); await settle();
    expect(useLiveVoice.getState().view.settings.modelsError).toBe('502: erro_codex_model_list: catálogo indisponível');
  });

  test('salva PUT inteiro, sem otimismo destrutivo ou dois saves em voo; erro preserva configuração', async () => {
    openA(); await settle();
    const pending = deferred<LiveVoiceSettingsReply['settings']>();
    api.save.mockReturnValueOnce(pending.promise).mockRejectedValueOnce(new LiveVoiceRequestError('save_failed'));
    const next = { ...reply().settings, voice: null, codex_account: 'next-account' };
    const saving = useLiveVoice.getState().saveSettings(next);
    await useLiveVoice.getState().saveSettings(next);
    expect(api.save).toHaveBeenCalledOnce();
    expect(api.save).toHaveBeenCalledWith(a, next);
    expect(useLiveVoice.getState().view.settings).toMatchObject({ saving: true, reply: reply() });
    pending.resolve(next); await saving;
    expect(useLiveVoice.getState().view.settings.reply?.settings).toEqual(next);
    await useLiveVoice.getState().saveSettings(reply().settings);
    expect(useLiveVoice.getState().view.settings).toMatchObject({ saving: false, saveError: 'save_failed', reply: { settings: next } });
  });

  test('contas/modelos/memória usam A; catálogo atrasado de outra conta é descartado', async () => {
    openA(); await settle();
    const oldModels = deferred<{ models: [] }>();
    api.models.mockReturnValueOnce(oldModels.promise).mockResolvedValueOnce({ models: [{ value: 'new-model', label: 'new-model' }] });
    useLiveVoice.getState().openSettings(); await settle();
    await useLiveVoice.getState().saveSettings(reply('new-account').settings);
    await settle();
    oldModels.resolve({ models: [] }); await settle();
    expect(api.accounts).toHaveBeenCalledWith(a);
    expect(api.memory).toHaveBeenCalledWith(a);
    expect(api.models).toHaveBeenLastCalledWith(a, 'codex', null, null, 'new-account');
    expect(useLiveVoice.getState().view.settings.models).toEqual([{ value: 'new-model', label: 'new-model' }]);
  });

  test('falhas de credenciais/modelos/memória não viram listas vazias sem causa', async () => {
    openA(); await settle();
    api.accounts.mockRejectedValueOnce(Object.assign(new Error('503: accounts_failed'), { status: 503 }));
    api.models.mockRejectedValueOnce(Object.assign(new Error('502: models_failed'), { status: 502 }));
    api.memory.mockRejectedValueOnce(new LiveVoiceRequestError('memory_failed'));
    useLiveVoice.getState().openSettings(); await settle();
    expect(useLiveVoice.getState().view.settings).toMatchObject({ error: '503: accounts_failed', modelsError: '502: models_failed' });
    expect(useLiveVoice.getState().view.memory.error).toBe('memory_failed');
    useLiveVoice.getState().openSettings(); await settle();
    expect(useLiveVoice.getState().view.settings.error).toBeNull();
    expect(useLiveVoice.getState().view.memory.error).toBeNull();
  });

  test('GET antigo não ressuscita memória apagada; memory_unknown remove a já inexistente', async () => {
    openA(); await settle();
    await useLiveVoice.getState().refreshMemory();
    const stale = deferred<VoiceMemory[]>();
    api.memory.mockReturnValueOnce(stale.promise);
    const reading = useLiveVoice.getState().refreshMemory();
    api.forget.mockRejectedValueOnce(Object.assign(new Error('already-gone'), { code: 'memory_unknown' }));
    await useLiveVoice.getState().forgetMemory(memory.id);
    stale.resolve([memory]); await reading;
    expect(api.forget).toHaveBeenCalledWith(a, memory.id);
    expect(useLiveVoice.getState().view.memory).toMatchObject({ loading: false, entries: [], forgetError: null });
  });

  test('state.memory atualiza ajustes abertos; fechamento da folha encerra refresh por estado', async () => {
    const instance = await startA();
    useLiveVoice.getState().openSettings(); await settle();
    const initial = api.memory.mock.calls.length;
    instance.handlers.state(state(1)); await settle();
    expect(api.memory).toHaveBeenCalledTimes(initial + 1);
    useLiveVoice.getState().dismiss();
    instance.handlers.state(state(2)); await settle();
    expect(api.memory).toHaveBeenCalledTimes(initial + 1);
  });
});
