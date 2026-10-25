import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createHmac } from 'node:crypto';
import { configureApi, decidirRota, esquecerRota, registrarDiag, type LiveVoiceHandlers, type Server } from '@hangar/core';
import { LiveVoiceCall } from './liveVoice';

vi.mock('@hangar/core', async importOriginal => ({
  ...await importOriginal<typeof import('@hangar/core')>(), registrarDiag: vi.fn(),
}));

const RuntimeLogger = (require('react-native-webrtc/lib/commonjs/Logger') as typeof import('react-native-webrtc/lib/typescript/Logger')).default;

const rtc = vi.hoisted(() => {
  const offer = vi.fn<() => Promise<{ type: string; sdp: string }>>();
  class Track {
    kind = 'audio'; enabled = true; readyState = 'live';
    onended: (() => void) | null = null;
    stop = vi.fn(() => { this.enabled = false; this.readyState = 'ended'; });
  }
  class Stream {
    track = new Track();
    getTracks = () => [this.track];
    getAudioTracks = () => [this.track];
    release = vi.fn();
  }
  class Peer {
    static all: Peer[] = [];
    connectionState = 'new';
    onconnectionstatechange: (() => void) | null = null;
    channel = { close: vi.fn() };
    addTrack = vi.fn();
    createDataChannel = vi.fn(() => this.channel);
    createOffer = vi.fn(() => offer());
    setLocalDescription = vi.fn(async (_sdp: unknown) => {});
    setRemoteDescription = vi.fn(async (_sdp: unknown) => {});
    getStats = vi.fn(async () => new Map<string, unknown>([
      ['mic', { id: 'mic', type: 'media-source', kind: 'audio', audioLevel: 0.03 }],
      ['speaker', { id: 'speaker', type: 'inbound-rtp', kind: 'audio', audioLevel: 0.02 }],
    ]));
    close = vi.fn(() => { this.connectionState = 'closed'; });
    constructor() { Peer.all.push(this); }
    connect() { this.connectionState = 'connected'; this.onconnectionstatechange?.(); }
  }
  return {
    Peer, Stream, offer, getUserMedia: vi.fn<() => Promise<Stream>>(),
    nativeStart: vi.fn<() => Promise<void>>(), nativeStop: vi.fn<() => Promise<void>>(),
    platform: { OS: 'ios', Version: 18 }, requestPermission: vi.fn<(permission: string) => Promise<string>>(),
    microphonePermission: vi.fn<(descriptor: { name: string }) => Promise<boolean>>(),
    clock: undefined as ReturnType<typeof setInterval> | undefined,
    ticks: new Set<() => void>(),
    listeners: new Set<(event: { reason: string }) => void>(), removes: [] as ReturnType<typeof vi.fn>[],
  };
});
vi.mock('react-native-webrtc', () => ({ RTCPeerConnection: rtc.Peer, mediaDevices: { getUserMedia: rtc.getUserMedia },
  permissions: { request: rtc.microphonePermission } }));
vi.mock('react-native', () => ({
  Platform: rtc.platform,
  PermissionsAndroid: { PERMISSIONS: { POST_NOTIFICATIONS: 'android.permission.POST_NOTIFICATIONS', BLUETOOTH_CONNECT: 'android.permission.BLUETOOTH_CONNECT' },
    RESULTS: { GRANTED: 'granted' }, request: rtc.requestPermission },
}));
vi.mock('../../../modules/live-voice-audio', () => ({ default: {
  start: rtc.nativeStart, stop: rtc.nativeStop,
  addListener: (name: string, callback: (event?: { reason: string }) => void) => {
    const listeners = name === 'tick' ? rtc.ticks : rtc.listeners;
    listeners.add(callback);
    const remove = vi.fn(() => { listeners.delete(callback); });
    rtc.removes.push(remove);
    return { remove };
  },
} }));

class Socket {
  static OPEN = 1;
  static all: Socket[] = [];
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  messages: Record<string, unknown>[] = [];
  send = vi.fn((data: string) => { this.messages.push(JSON.parse(data)); });
  close = vi.fn(() => { this.readyState = 3; });
  constructor(readonly url: string, readonly protocols?: string[], readonly options?: { headers: { Origin: string } }) { Socket.all.push(this); }
  open() { this.readyState = Socket.OPEN; this.onopen?.(); }
  receive(message: object) { this.onmessage?.({ data: JSON.stringify(message) }); }
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const server: Server = { id: 'a', label: 'A', baseUrl: 'https://a.test', token: 'token-a' };
const calls: LiveVoiceCall[] = [];
const cleanupFailures = new Set<LiveVoiceCall>();
function makeCall() {
  const handlers: LiveVoiceHandlers = {
    phase: vi.fn(), state: vi.fn(), failed: vi.fn(), taken: vi.fn(), levels: vi.fn(),
    switchTo: vi.fn(() => true), actions: vi.fn(() => [{ id: 'sessions', label: 'sessions', description: 'list' }]),
    runAction: vi.fn(() => true as const),
  };
  const call = new LiveVoiceCall(handlers);
  calls.push(call);
  return { call, handlers };
}
async function openCall() {
  const value = makeCall();
  await value.call.start(server, { server: '', name: 'chat' });
  const socket = Socket.all.at(-1)!;
  const pc = rtc.Peer.all.at(-1)!;
  socket.open();
  return { ...value, socket, pc };
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(registrarDiag).mockClear();
  vi.setSystemTime(0);
  vi.stubGlobal('WebSocket', Socket);
  rtc.Peer.all = []; Socket.all = [];
  rtc.offer.mockReset().mockResolvedValue({ type: 'offer', sdp: 'offer-sdp' });
  rtc.getUserMedia.mockReset().mockImplementation(async () => new rtc.Stream());
  rtc.platform.OS = 'ios'; rtc.platform.Version = 18;
  rtc.requestPermission.mockReset().mockResolvedValue('granted');
  rtc.microphonePermission.mockReset().mockResolvedValue(true);
  rtc.nativeStart.mockReset().mockImplementation(async () => {
    if (rtc.clock === undefined) rtc.clock = setInterval(() => { for (const tick of rtc.ticks) tick(); }, 100);
  });
  rtc.nativeStop.mockReset().mockImplementation(async () => { clearInterval(rtc.clock); rtc.clock = undefined; });
  rtc.listeners.clear(); rtc.ticks.clear(); rtc.removes = [];
  configureApi({ getBaseUrl: () => 'https://active-b.test', getToken: () => 'token-b', origin: null,
    onUnauthorized: vi.fn(), createEventSource: () => { throw new Error('unexpected_stream'); } });
});
afterEach(async () => {
  for (const call of calls.splice(0)) {
    if (cleanupFailures.delete(call)) await expect(call.stop()).rejects.toMatchObject({ code: 'audio_restore' });
    else await call.stop();
  }
  clearInterval(rtc.clock); rtc.clock = undefined;
  vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks();
});

describe('Origin HTTP da conexão efetiva de voz', () => {
  afterEach(() => esquecerRota(server.id));
  it.each([
    { baseUrl: 'http://voice.test:8765/prefix', lan: undefined, url: 'ws://voice.test:8765/prefix/api/voice?token=token-a', origin: 'http://voice.test:8765' },
    { baseUrl: 'https://voice.test:8443/prefix', lan: undefined, url: 'wss://voice.test:8443/prefix/api/voice?token=token-a', origin: 'https://voice.test:8443' },
    { baseUrl: 'https://remote.test/prefix', lan: { id: 'voice-lan', url: 'http://192.168.1.10:8765/local' }, url: 'ws://192.168.1.10:8765/local/api/voice?token=token-a', origin: 'http://192.168.1.10:8765' },
  ])('envia Origin explícita $origin ao abrir $url', async ({ baseUrl, lan, url, origin }) => {
    const target = { ...server, baseUrl, lan };
    if (lan) {
      vi.stubGlobal('fetch', vi.fn(async (address: string) => {
        if (!address.includes('/prova?')) return new Promise(() => {});
        const challenge = new URL(address).searchParams.get('desafio')!;
        return { ok: true, json: async () => ({ identificador: 'voice-lan', prova: createHmac('sha256', 'token-a').update(`${challenge}|voice-lan`).digest('hex') }) };
      }));
      await decidirRota(target);
    }
    const { call } = makeCall();
    await call.start(target, null);
    const socket = Socket.all.at(-1)!;
    expect(socket.url).toBe(url);
    expect(socket.options).toEqual({ headers: { Origin: origin } });
  });
});

describe('logs WebRTC pela abertura pública da chamada', () => {
  let previousDebug: string | undefined;
  beforeEach(() => { previousDebug = process.env.DEBUG; });
  afterEach(() => {
    RuntimeLogger.enable(previousDebug ?? '');
    if (previousDebug === undefined) delete process.env.DEBUG;
    else process.env.DEBUG = previousDebug;
  });

  it('release bloqueia DEBUG, mas mantém INFO, WARN e ERROR do logger real', async () => {
    const output = vi.spyOn(console, 'log').mockImplementation(() => {});
    RuntimeLogger.enable(`${RuntimeLogger.ROOT_PREFIX}:*`);
    const logger = new RuntimeLogger('pc');
    vi.stubGlobal('__DEV__', false);
    const { call, handlers } = makeCall();
    await call.start(server, null);
    expect(handlers.failed).not.toHaveBeenCalled();
    logger.debug('getStats-release-marker');
    logger.info('info-release-marker');
    logger.warn('warn-release-marker');
    logger.error('error-release-marker');
    const printed = output.mock.calls.map(args => args.join(' ')).join('\n');
    expect(printed).not.toContain('getStats-release-marker');
    expect(printed).toContain('info-release-marker');
    expect(printed).toContain('warn-release-marker');
    expect(printed).toContain('error-release-marker');
  });

  it('desenvolvimento conserva DEBUG e não altera a configuração do logger', async () => {
    const output = vi.spyOn(console, 'log').mockImplementation(() => {});
    RuntimeLogger.enable(`${RuntimeLogger.ROOT_PREFIX}:*`);
    const logger = new RuntimeLogger('pc');
    const enable = vi.spyOn(RuntimeLogger, 'enable');
    vi.stubGlobal('__DEV__', true);
    const { call, handlers } = makeCall();
    await call.start(server, null);
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(enable).not.toHaveBeenCalled();
    logger.debug('getStats-dev-marker');
    expect(output.mock.calls.map(args => args.join(' ')).join('\n')).toContain('getStats-dev-marker');
  });
});

describe('permissões fora do orçamento de abertura', () => {
  it.each([
    { os: 'android', stages: ['notification'] },
    { os: 'android', stages: ['bluetooth'] },
    { os: 'android', stages: ['microphone'] },
    { os: 'android', stages: ['notification', 'bluetooth', 'microphone'] },
    { os: 'ios', stages: ['microphone'] },
  ])('$os: espera mais de 60s por $stages e só depois captura e fica live', async ({ os, stages }) => {
    rtc.platform.OS = os; rtc.platform.Version = os === 'android' ? 33 : 18;
    const waiting = new Map(stages.map(stage => [stage, { entered: deferred<void>(), grant: deferred<void>() }]));
    rtc.requestPermission.mockImplementation(async permission => {
      const stage = permission === 'android.permission.POST_NOTIFICATIONS' ? 'notification' : 'bluetooth';
      const gate = waiting.get(stage);
      if (gate) { gate.entered.resolve(); await gate.grant.promise; }
      return 'granted';
    });
    rtc.microphonePermission.mockImplementation(async () => {
      const gate = waiting.get('microphone');
      if (gate) { gate.entered.resolve(); await gate.grant.promise; }
      return true;
    });
    const { call, handlers } = makeCall();
    const starting = call.start(server, null);
    for (const gate of waiting.values()) {
      await gate.entered.promise;
      await vi.advanceTimersByTimeAsync(90000);
      expect(handlers.phase).toHaveBeenLastCalledWith('connecting');
      expect(handlers.failed).not.toHaveBeenCalled();
      expect(rtc.getUserMedia).not.toHaveBeenCalled();
      expect(rtc.nativeStart).not.toHaveBeenCalled();
      expect(Socket.all).toHaveLength(0);
      gate.grant.resolve();
    }
    await starting;
    expect(rtc.microphonePermission).toHaveBeenCalledExactlyOnceWith({ name: 'microphone' });
    expect(rtc.getUserMedia).toHaveBeenCalledExactlyOnceWith({ audio: true, video: false });
    Socket.all[0].open(); rtc.Peer.all[0].connect();
    await vi.advanceTimersByTimeAsync(1000);
    expect(handlers.phase).toHaveBeenLastCalledWith('live');
    expect(handlers.failed).not.toHaveBeenCalled();
  });

  it.each(['android', 'ios'])('%s: microfone negado ou erro conserva a causa e não abre captura', async os => {
    rtc.platform.OS = os; rtc.platform.Version = os === 'android' ? 33 : 18;
    rtc.microphonePermission.mockResolvedValueOnce(false).mockRejectedValueOnce(new Error('microphone_request_failed'));
    const { call, handlers } = makeCall();
    await call.start(server, null);
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('microphone', undefined);
    await call.start(server, null);
    expect(handlers.failed).toHaveBeenLastCalledWith('microphone', 'microphone_request_failed');
    expect(handlers.failed).toHaveBeenCalledTimes(2);
    expect(rtc.getUserMedia).not.toHaveBeenCalled();
    expect(rtc.nativeStart).not.toHaveBeenCalled();
    expect(Socket.all).toHaveLength(0);
  });

  it('permissão demorada concede 60s novos para a captura pendente, não tempo ilimitado', async () => {
    const permission = deferred<boolean>(), entered = deferred<void>(), capturing = deferred<void>();
    rtc.microphonePermission.mockImplementationOnce(() => { entered.resolve(); return permission.promise; });
    rtc.getUserMedia.mockImplementationOnce(() => { capturing.resolve(); return new Promise<InstanceType<typeof rtc.Stream>>(() => {}); });
    const { call, handlers } = makeCall();
    const starting = call.start(server, null); await entered.promise;
    await vi.advanceTimersByTimeAsync(90000);
    expect(handlers.failed).not.toHaveBeenCalled();
    permission.resolve(true); await capturing.promise;
    await vi.advanceTimersByTimeAsync(59999);
    expect(handlers.failed).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1); await starting;
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('opening_timeout', undefined);
    expect(rtc.nativeStart).not.toHaveBeenCalled();
    expect(Socket.all).toHaveLength(0);
  });

  it.each([
    { stage: 'notification', os: 'android', version: 33 },
    { stage: 'bluetooth', os: 'android', version: 31 },
    { stage: 'microphone', os: 'android', version: 30 },
    { stage: 'microphone', os: 'ios', version: 18 },
  ])('$os $stage: stop e nova geração ignoram concessão tardia', async ({ stage, os, version }) => {
    rtc.platform.OS = os; rtc.platform.Version = version;
    const { call, handlers } = makeCall();
    for (const replace of [false, true]) {
      const entered = deferred<void>(), android = deferred<string>(), microphone = deferred<boolean>();
      let granted: Promise<unknown>, grant: () => void;
      if (stage === 'microphone') {
        rtc.microphonePermission.mockImplementationOnce(() => { entered.resolve(); return microphone.promise; });
        granted = microphone.promise; grant = () => microphone.resolve(true);
      } else {
        rtc.requestPermission.mockImplementationOnce(() => { entered.resolve(); return android.promise; });
        granted = android.promise; grant = () => android.resolve('granted');
      }
      const starting = call.start(server, null); await entered.promise;
      await vi.advanceTimersByTimeAsync(90000);
      if (replace) await call.start(server, null);
      else await call.stop();
      await starting;
      const captures = rtc.getUserMedia.mock.calls.length, sockets = Socket.all.length;
      grant(); await granted;
      expect(rtc.getUserMedia).toHaveBeenCalledTimes(captures);
      expect(Socket.all).toHaveLength(sockets);
      expect(handlers.failed).not.toHaveBeenCalled();
      if (replace) {
        Socket.all.at(-1)!.open(); rtc.Peer.all.at(-1)!.connect();
        expect(handlers.phase).toHaveBeenLastCalledWith('live');
        expect(Socket.all.at(-1)!.close).not.toHaveBeenCalled();
      }
      await call.stop();
    }
  });
});

describe('LiveVoiceCall público', () => {
  it.each(['URL', 'WebSocket'] as const)('falha do construtor %s não publica segredo cru nem codificado', async constructor => {
    const secret = 'private/value?&= á';
    const error = new Error(`wss://a.test/api/voice?token=${secret}; token=${encodeURIComponent(secret)}`);
    if (constructor === 'URL') vi.stubGlobal('URL', class { constructor() { throw error; } });
    else vi.stubGlobal('WebSocket', class { constructor() { throw error; } });
    const { call, handlers } = makeCall();
    await call.start({ ...server, token: secret }, null);
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('failed', constructor === 'URL' ? 'invalid_server_url' : 'websocket_start_failed');
    await call.stop();
    const published = JSON.stringify({ errors: vi.mocked(handlers.failed).mock.calls, diagnostics: vi.mocked(registrarDiag).mock.calls });
    expect(published).not.toContain(secret);
    expect(published).not.toContain(encodeURIComponent(secret));
  });

  it('conversor central não repassa token em erro de envio ou do servidor', async () => {
    const { call, socket, handlers } = await openCall();
    socket.send.mockImplementationOnce(() => { throw new Error('send token=private%2Fvalue&other=value'); });
    call.setMode('direto');
    expect(handlers.failed).toHaveBeenCalledWith('connection_lost', 'token=[redacted]');
    await call.start(server, null);
    Socket.all.at(-1)!.receive({ type: 'error', code: 'failed', detail: 'request token=another-secret' });
    expect(handlers.failed).toHaveBeenLastCalledWith('failed', 'token=[redacted]');
  });

  it('stop devolve a falha da restauração nativa e não a repete como falha genérica', async () => {
    const { call, socket, handlers } = await openCall();
    cleanupFailures.add(call);
    rtc.nativeStop.mockRejectedValueOnce(new Error('private-native-restore'));
    await expect(call.stop()).rejects.toMatchObject({ code: 'audio_restore' });
    expect(handlers.phase).toHaveBeenLastCalledWith('closed');
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('audio_restore', undefined);
    expect(socket.messages.filter(message => message.type === 'stop')).toHaveLength(1);
  });

  it.each(['taken', 'error'] as const)('limpeza rejeitada após %s é tratada sem substituir a causa original', async terminal => {
    const { call, socket, handlers } = await openCall();
    cleanupFailures.add(call);
    rtc.nativeStop.mockRejectedValueOnce(new Error('restore-failed'));
    socket.receive(terminal === 'taken' ? { type: 'taken' } : { type: 'error', code: 'account_missing', detail: 'chosen-account' });
    await expect(call.stop()).rejects.toMatchObject({ code: 'audio_restore' });
    if (terminal === 'taken') {
      expect(handlers.taken).toHaveBeenCalledOnce();
      expect(handlers.failed).not.toHaveBeenCalled();
    } else expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('account_missing', 'chosen-account');
    expect(socket.messages.some(message => message.type === 'stop')).toBe(false);
  });

  it('disconnected tolera 5s, mede microfone e reconecta sem repetir live', async () => {
    const { pc, socket, handlers } = await openCall(); pc.connect();
    pc.connectionState = 'disconnected'; pc.onconnectionstatechange?.();
    await vi.advanceTimersByTimeAsync(4900);
    expect(pc.close).not.toHaveBeenCalled();
    expect(socket.messages).toContainEqual({ type: 'level', input: 0.03 });
    expect(handlers.failed).not.toHaveBeenCalled();
    pc.connect();
    await vi.advanceTimersByTimeAsync(200);
    expect(pc.close).not.toHaveBeenCalled();
    expect(socket.messages.filter(message => message.type === 'live')).toHaveLength(1);
    expect(vi.mocked(handlers.phase).mock.calls.filter(([phase]) => phase === 'live')).toHaveLength(1);
  });

  it('disconnected repetido não reinicia tolerância; tick nativo decide mesmo sem timer JS', async () => {
    const { pc, socket, handlers } = await openCall(); pc.connect();
    pc.connectionState = 'disconnected'; pc.onconnectionstatechange?.();
    vi.setSystemTime(4000); pc.onconnectionstatechange?.();
    expect(pc.close).not.toHaveBeenCalled();
    vi.setSystemTime(5000);
    for (const tick of rtc.ticks) tick();
    expect(pc.close).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(0);
    expect(socket.messages.filter(message => message.type === 'offer')).toHaveLength(2);
    expect(handlers.failed).not.toHaveBeenCalled();
  });

  it.each(['failed', 'closed'])('RTC %s depois de ao vivo manda oferta nova no mesmo socket e segue a chamada', async state => {
    const { pc, socket, handlers } = await openCall(); pc.connect();
    pc.connectionState = state; pc.onconnectionstatechange?.();
    expect(pc.close).toHaveBeenCalledOnce();
    expect(handlers.phase).toHaveBeenLastCalledWith('connecting');
    await vi.advanceTimersByTimeAsync(0);
    const second = rtc.Peer.all.at(-1)!;
    expect(second).not.toBe(pc);
    expect(socket.messages.filter(message => message.type === 'offer')).toHaveLength(2);
    socket.receive({ type: 'answer', sdp: 'answer-2' });
    await vi.advanceTimersByTimeAsync(0);
    expect(second.setRemoteDescription).toHaveBeenCalledWith({ type: 'answer', sdp: 'answer-2' });
    second.connect();
    expect(socket.messages.filter(message => message.type === 'live')).toHaveLength(2);
    expect(handlers.phase).toHaveBeenLastCalledWith('live');
    await vi.advanceTimersByTimeAsync(10_500);
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(socket.messages.some(message => message.type === 'stop')).toBe(false);
    expect(Socket.all).toHaveLength(1);
  });

  it('RTC que falha antes de ao vivo encerra com connection_lost', async () => {
    const { pc, socket, handlers } = await openCall();
    pc.connectionState = 'failed'; pc.onconnectionstatechange?.();
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('connection_lost', undefined);
    expect(socket.messages.filter(message => message.type === 'offer')).toHaveLength(1);
  });

  it('conexão nova que falha antes de conectar encerra em vez de tentar de novo', async () => {
    const { pc, socket, handlers } = await openCall(); pc.connect();
    pc.connectionState = 'failed'; pc.onconnectionstatechange?.();
    await vi.advanceTimersByTimeAsync(0);
    const second = rtc.Peer.all.at(-1)!;
    second.connectionState = 'failed'; second.onconnectionstatechange?.();
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('connection_lost', undefined);
    expect(socket.messages.filter(message => message.type === 'offer')).toHaveLength(2);
  });

  it('callback e tolerância da desconexão antiga não encerram tentativa nova', async () => {
    const { call, pc, handlers } = await openCall(); pc.connect();
    const oldChange = pc.onconnectionstatechange!;
    pc.connectionState = 'disconnected'; oldChange();
    await call.start(server, null);
    const current = rtc.Peer.all.at(-1)!; Socket.all.at(-1)!.open(); current.connect();
    oldChange();
    await vi.advanceTimersByTimeAsync(5100);
    expect(current.close).not.toHaveBeenCalled();
    expect(handlers.failed).not.toHaveBeenCalled();
  });

  it('captura que nunca resolve expira em 60s e start conclui sem esperar a captura', async () => {
    const entered = deferred<void>(), pending = deferred<InstanceType<typeof rtc.Stream>>();
    rtc.getUserMedia.mockImplementationOnce(() => { entered.resolve(); return pending.promise; });
    const { call, handlers } = makeCall();
    const starting = call.start(server, null);
    await entered.promise;
    await vi.advanceTimersByTimeAsync(60000);
    await starting;
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('opening_timeout', undefined);
    expect(Socket.all).toHaveLength(0);
    expect(rtc.nativeStart).not.toHaveBeenCalled();
    const media = new rtc.Stream(), released = deferred<void>();
    media.release.mockImplementationOnce(() => { released.resolve(); });
    pending.resolve(media); await released.promise;
    expect(media.track.stop).toHaveBeenCalledOnce();
    expect(rtc.nativeStart).not.toHaveBeenCalled();
    expect(Socket.all).toHaveLength(0);
  });

  it('oferta pendente conta no prazo absoluto e é liberada sem abrir socket tardio', async () => {
    const entered = deferred<void>(), offer = deferred<{ type: string; sdp: string }>();
    rtc.offer.mockImplementationOnce(() => { entered.resolve(); return offer.promise; });
    const { call, handlers } = makeCall();
    const starting = call.start(server, null); await entered.promise;
    await vi.advanceTimersByTimeAsync(60000); await starting;
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('opening_timeout', undefined);
    const pc = rtc.Peer.all[0];
    expect(pc.close).toHaveBeenCalledOnce();
    expect(rtc.nativeStop).toHaveBeenCalledOnce();
    offer.resolve({ type: 'offer', sdp: 'late-offer' }); await offer.promise;
    expect(pc.setLocalDescription).not.toHaveBeenCalled();
    expect(Socket.all).toHaveLength(0);
  });

  it('prazo inclui limpeza anterior pendente e não fica esperando Native.stop', async () => {
    const { call, handlers } = await openCall();
    const entered = deferred<void>(), cleanup = deferred<void>();
    rtc.nativeStop.mockImplementationOnce(() => { entered.resolve(); return cleanup.promise; });
    const starting = call.start(server, null); await entered.promise;
    await vi.advanceTimersByTimeAsync(60000); await starting;
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('opening_timeout', undefined);
    expect(rtc.getUserMedia).toHaveBeenCalledTimes(1);
    cleanup.resolve(); await call.stop();
    expect(Socket.all).toHaveLength(1);
  });

  it('Native.start tardio é restaurado depois do timeout sem iniciar RTC', async () => {
    const entered = deferred<void>(), native = deferred<void>(), stopped = deferred<void>();
    rtc.nativeStart.mockImplementationOnce(() => { entered.resolve(); return native.promise; });
    rtc.nativeStop.mockImplementationOnce(async () => { stopped.resolve(); });
    const { call, handlers } = makeCall();
    const starting = call.start(server, null); await entered.promise;
    vi.setSystemTime(60000);
    for (const tick of rtc.ticks) tick();
    await starting;
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('opening_timeout', undefined);
    expect(rtc.Peer.all).toHaveLength(0);
    native.resolve(); await stopped.promise;
    expect(rtc.nativeStop).toHaveBeenCalledOnce();
    expect(Socket.all).toHaveLength(0);
  });

  it('retorno tardio confere deadline mesmo quando timer JS não teve oportunidade de rodar', async () => {
    const entered = deferred<void>(), media = deferred<InstanceType<typeof rtc.Stream>>();
    rtc.getUserMedia.mockImplementationOnce(() => { entered.resolve(); return media.promise; });
    const { call, handlers } = makeCall();
    const starting = call.start(server, null); await entered.promise;
    vi.setSystemTime(60000);
    const stream = new rtc.Stream(), released = deferred<void>();
    stream.release.mockImplementationOnce(() => { released.resolve(); });
    media.resolve(stream); await released.promise; await starting;
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('opening_timeout', undefined);
    expect(rtc.nativeStart).not.toHaveBeenCalled();
  });

  it('chamada live não expira aos 60s da abertura', async () => {
    const { pc, socket, handlers } = await openCall(); pc.connect();
    for (let period = 0; period < 4; period++) {
      await vi.advanceTimersByTimeAsync(20000); socket.receive({ type: 'pong' });
    }
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(pc.close).not.toHaveBeenCalled();
  });

  it('liga A com B ativo, captura só áudio e anuncia apenas as ferramentas atendidas', async () => {
    const { socket, pc, call } = await openCall();
    expect(socket.url).toBe('wss://a.test/api/voice?token=token-a');
    expect(rtc.getUserMedia).toHaveBeenCalledWith({ audio: true, video: false });
    expect(rtc.nativeStart).toHaveBeenCalledWith(expect.objectContaining({ title: expect.any(String), body: expect.any(String), stopLabel: expect.any(String) }));
    const stream = await rtc.getUserMedia.mock.results[0].value;
    expect(stream.track.enabled).toBe(false);
    expect(pc.createDataChannel).toHaveBeenCalledWith('oai-events');
    expect(pc.channel.close).not.toHaveBeenCalled();
    expect(socket.messages[0]).toMatchObject({ type: 'hello', client: 'mobile', screen: { server: '', name: 'chat' },
      caps: ['switch_session', 'hangar_actions', 'hangar_action'] });
    expect(socket.messages[1]).toEqual({ type: 'offer', sdp: 'offer-sdp' });
    pc.connect();
    expect(stream.track.enabled).toBe(true);
    expect(socket.messages).toContainEqual({ type: 'live' });
    await call.stop();
    expect(stream.track.stop).toHaveBeenCalledTimes(1);
    expect(stream.release).toHaveBeenCalledTimes(1);
    expect(pc.close).toHaveBeenCalledTimes(1);
    expect(pc.channel.close).toHaveBeenCalledTimes(1);
    expect(socket.close).toHaveBeenCalledTimes(1);
    expect(rtc.nativeStop).toHaveBeenCalledTimes(1);
    expect(rtc.removes.every(remove => remove.mock.calls.length === 1)).toBe(true);
  });

  it('Android 33 exige notificação antes de abrir microfone ou serviço', async () => {
    rtc.platform.OS = 'android'; rtc.platform.Version = 33;
    rtc.requestPermission.mockResolvedValue('denied');
    const { call, handlers } = makeCall();
    await call.start(server, null);
    expect(rtc.requestPermission).toHaveBeenCalledWith('android.permission.POST_NOTIFICATIONS');
    expect(rtc.getUserMedia).not.toHaveBeenCalled();
    expect(rtc.nativeStart).not.toHaveBeenCalled();
    expect(handlers.failed).toHaveBeenCalledWith('notifications_permission', undefined);
  });

  it('Android 32 pede Bluetooth, mas não notificação; iOS não pede nenhum dos dois', async () => {
    rtc.platform.OS = 'android'; rtc.platform.Version = 32;
    const { call } = await openCall();
    expect(rtc.requestPermission).toHaveBeenCalledExactlyOnceWith('android.permission.BLUETOOTH_CONNECT');
    expect(rtc.requestPermission).not.toHaveBeenCalledWith('android.permission.POST_NOTIFICATIONS');
    await call.stop();
    rtc.requestPermission.mockClear();
    rtc.platform.OS = 'ios'; rtc.platform.Version = 18;
    await openCall();
    expect(rtc.requestPermission).not.toHaveBeenCalled();
  });

  it('Android anterior a 31 não pede Bluetooth nem notificação', async () => {
    rtc.platform.OS = 'android'; rtc.platform.Version = 30;
    await openCall();
    expect(rtc.requestPermission).not.toHaveBeenCalled();
  });

  it.each(['granted', 'denied', 'never_ask_again'])('Bluetooth %s não impede a chamada e resolve antes da captura', async (permission) => {
    rtc.platform.OS = 'android'; rtc.platform.Version = 31;
    const pending = deferred<string>(), entered = deferred<void>();
    rtc.requestPermission.mockImplementationOnce(() => { entered.resolve(); return pending.promise; });
    const { call, handlers } = makeCall();
    const starting = call.start(server, null);
    await entered.promise;
    expect(rtc.requestPermission).toHaveBeenCalledWith('android.permission.BLUETOOTH_CONNECT');
    expect(rtc.getUserMedia).not.toHaveBeenCalled();
    expect(rtc.nativeStart).not.toHaveBeenCalled();
    pending.resolve(permission);
    await starting;
    expect(rtc.getUserMedia).toHaveBeenCalledTimes(1);
    expect(rtc.nativeStart).toHaveBeenCalledTimes(1);
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(Socket.all).toHaveLength(1);
  });

  it('cancelar enquanto pede Bluetooth impede captura e serviço após a resposta tardia', async () => {
    rtc.platform.OS = 'android'; rtc.platform.Version = 31;
    const pending = deferred<string>(), entered = deferred<void>();
    rtc.requestPermission.mockImplementationOnce(() => { entered.resolve(); return pending.promise; });
    const { call, handlers } = makeCall();
    const starting = call.start(server, null);
    await entered.promise;
    await call.stop();
    await starting;
    pending.resolve('granted');
    await pending.promise;
    expect(rtc.getUserMedia).not.toHaveBeenCalled();
    expect(rtc.nativeStart).not.toHaveBeenCalled();
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(Socket.all).toHaveLength(0);
  });

  it('exceção ao pedir Bluetooth é visível sem virar erro de notificação', async () => {
    rtc.platform.OS = 'android'; rtc.platform.Version = 33;
    rtc.requestPermission.mockImplementation(async permission => {
      if (permission === 'android.permission.BLUETOOTH_CONNECT') throw new Error('bluetooth_request_failed');
      return 'granted';
    });
    const { call, handlers } = makeCall();
    await call.start(server, null);
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('failed', 'bluetooth_request_failed');
    expect(rtc.getUserMedia).not.toHaveBeenCalled();
    expect(rtc.nativeStart).not.toHaveBeenCalled();
  });

  it('desligar com captura pendente libera a mídia tardia sem conectar', async () => {
    const pending = deferred<InstanceType<typeof rtc.Stream>>(), entered = deferred<void>();
    rtc.getUserMedia.mockImplementationOnce(() => { entered.resolve(); return pending.promise; });
    const { call, handlers } = makeCall();
    const starting = call.start(server, null);
    await entered.promise;
    await call.stop();
    await starting;
    const stream = new rtc.Stream(), released = deferred<void>();
    stream.release.mockImplementationOnce(() => { released.resolve(); });
    pending.resolve(stream);
    await released.promise;
    expect(stream.track.stop).toHaveBeenCalledTimes(1);
    expect(stream.release).toHaveBeenCalledTimes(1);
    expect(Socket.all).toHaveLength(0);
    expect(handlers.failed).not.toHaveBeenCalled();
  });

  it('answer tardia da tentativa antiga não derruba nem muda a chamada nova', async () => {
    const { call, socket, pc, handlers } = await openCall();
    const answer = deferred<void>();
    pc.setRemoteDescription.mockReturnValueOnce(answer.promise);
    const oldMessage = socket.onmessage!;
    socket.receive({ type: 'answer', sdp: 'old-answer' });
    await call.start(server, null);
    const current = Socket.all.at(-1)!; current.open();
    rtc.Peer.all.at(-1)!.connect();
    answer.reject(new Error('closed_peer'));
    await Promise.resolve(); await Promise.resolve();
    oldMessage({ data: JSON.stringify({ type: 'closed' }) });
    expect(current.close).not.toHaveBeenCalled();
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(handlers.phase).toHaveBeenLastCalledWith('live');
  });

  it('RMS enviado é cru; barra tem ganho e mudo manda zero mesmo com stats antigos', async () => {
    const { call, socket, pc, handlers } = await openCall(); pc.connect();
    await vi.advanceTimersByTimeAsync(100);
    expect(socket.messages).toContainEqual({ type: 'level', input: 0.03 });
    expect(handlers.levels).toHaveBeenLastCalledWith({ input: 0.16, output: 0.1 });
    call.setMuted(true);
    await vi.advanceTimersByTimeAsync(100);
    expect(socket.messages.at(-1)).toEqual({ type: 'level', input: 0 });
    expect(handlers.levels).toHaveBeenLastCalledWith({ input: 0, output: 0.1 });
  });

  it('mudo envia zero conhecido mesmo com getStats anterior ainda pendente', async () => {
    const { call, socket, pc, handlers } = await openCall();
    const pending = deferred<Map<string, unknown>>();
    pc.getStats.mockReturnValue(pending.promise);
    pc.connect();
    await vi.advanceTimersByTimeAsync(100);
    call.setMuted(true);
    await vi.advanceTimersByTimeAsync(10000);
    expect(socket.messages).toContainEqual({ type: 'level', input: 0 });
    expect(handlers.failed).not.toHaveBeenCalled();
    pending.resolve(new Map());
    await vi.advanceTimersByTimeAsync(0);
  });

  it('delta de energia mede entrada/saída sem inventar zero na primeira amostra', async () => {
    const { pc, socket } = await openCall();
    pc.getStats.mockResolvedValueOnce(new Map([
      ['mic', { id: 'mic', type: 'media-source', kind: 'audio', totalAudioEnergy: 1, totalSamplesDuration: 10 }],
      ['speaker', { id: 'speaker', type: 'inbound-rtp', kind: 'audio', totalAudioEnergy: 2, totalSamplesDuration: 10 }],
    ])).mockResolvedValueOnce(new Map([
      ['mic', { id: 'mic', type: 'media-source', kind: 'audio', totalAudioEnergy: 1.0025, totalSamplesDuration: 11 }],
      ['speaker', { id: 'speaker', type: 'inbound-rtp', kind: 'audio', totalAudioEnergy: 2.01, totalSamplesDuration: 11 }],
    ]));
    pc.connect();
    await vi.advanceTimersByTimeAsync(100);
    expect(socket.messages.filter(m => m.type === 'level')).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(100);
    expect(socket.messages.find(m => m.type === 'level')?.input).toBeCloseTo(0.05);
  });

  it('entrada medida com saída ainda ausente mantém a chamada sem erro', async () => {
    const { pc, socket, handlers } = await openCall();
    pc.getStats.mockResolvedValue(new Map([
      ['mic', { id: 'mic', type: 'media-source', kind: 'audio', audioLevel: 0 }],
    ]));
    pc.connect();
    await vi.advanceTimersByTimeAsync(12000);
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(socket.messages).toContainEqual({ type: 'level', input: 0 });
    expect(handlers.levels).toHaveBeenLastCalledWith({ input: 0, output: 0 });
  });

  it('stats sem medição viram erro visível, sem level inventado', async () => {
    const { pc, socket, handlers } = await openCall();
    pc.getStats.mockResolvedValue(new Map()); pc.connect();
    await vi.advanceTimersByTimeAsync(10000);
    expect(handlers.failed).toHaveBeenCalledWith('audio_levels_unavailable', expect.any(String));
    expect(socket.messages.some(m => m.type === 'level')).toBe(false);
    expect(socket.messages).toContainEqual({ type: 'stop' });
  });

  it('getStats rejeitado enquanto mudo conserva zero conhecido sem erro de entrada', async () => {
    const { call, pc, socket, handlers } = await openCall();
    pc.getStats.mockRejectedValue(new Error('unavailable-while-muted'));
    pc.connect(); call.setMuted(true);
    await vi.advanceTimersByTimeAsync(10000);
    expect(socket.messages).toContainEqual({ type: 'level', input: 0 });
    expect(handlers.failed).not.toHaveBeenCalled();
  });

  it('getStats rejeitado mostra causa estável sem vazar IDs do diagnóstico nativo', async () => {
    const { pc, handlers } = await openCall();
    pc.getStats.mockRejectedValue(new Error('track-secret-native-id'));
    pc.connect();
    await vi.advanceTimersByTimeAsync(100);
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('audio_levels_unavailable', 'getStats_failed');
  });

  it('ping parte da criação do socket e corta VPN que nunca abriu em 30s', async () => {
    const { call, handlers } = makeCall();
    await call.start(server, null);
    await vi.advanceTimersByTimeAsync(30000);
    expect(handlers.failed).toHaveBeenCalledWith('connection_lost', undefined);
    expect(Socket.all[0].close).toHaveBeenCalledTimes(1);
    expect(Socket.all[0].messages.some(m => m.type === 'stop')).toBe(false);
  });

  it('ping a cada 10s e pong renova o prazo sem fluxo de áudio no WS', async () => {
    const { socket, handlers, pc } = await openCall();
    pc.connect();
    await vi.advanceTimersByTimeAsync(20000);
    expect(socket.messages.filter(m => m.type === 'ping')).toHaveLength(2);
    socket.receive({ type: 'pong' });
    await vi.advanceTimersByTimeAsync(20000);
    expect(handlers.failed).not.toHaveBeenCalled();
  });

  it('taken limpa sem stop e invalida callbacks guardados', async () => {
    const { socket, handlers } = await openCall();
    const oldClose = socket.onclose!;
    socket.receive({ type: 'taken' }); oldClose();
    await Promise.resolve(); await Promise.resolve();
    expect(handlers.taken).toHaveBeenCalledTimes(1);
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(socket.messages.some(m => m.type === 'stop')).toBe(false);
    expect(socket.close).toHaveBeenCalledTimes(1);
  });

  it('erro fatal seguido de closed/close conserva causa e detalhe', async () => {
    const { socket, handlers } = await openCall();
    const oldMessage = socket.onmessage!, oldClose = socket.onclose!;
    socket.receive({ type: 'error', code: 'account_missing', detail: 'chosen-account' });
    oldMessage({ data: JSON.stringify({ type: 'closed' }) }); oldClose();
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('account_missing', 'chosen-account');
    expect(socket.messages.some(m => m.type === 'stop')).toBe(false);
  });

  it.each(['error', 'close'] as const)('socket %s mostra falha sem encerrar organizador', async (event) => {
    const { socket, handlers } = await openCall();
    if (event === 'error') socket.onerror?.(); else socket.onclose?.();
    expect(handlers.failed).toHaveBeenCalledWith('connection_lost', undefined);
    expect(socket.messages.some(m => m.type === 'stop')).toBe(false);
  });

  it('ferramenta desconhecida sempre recebe tool_result; modo só manda controle', async () => {
    const fetch = vi.spyOn(globalThis, 'fetch');
    const { call, socket, handlers } = await openCall();
    socket.receive({ type: 'tool', call: 9, name: 'read_screen', args: {} });
    socket.receive({ type: 'tool', call: 10, name: 'switch_session', args: { server: 'peer', base_url: 'https://peer.test', name: 'other' } });
    socket.receive({ type: 'tool', call: 11, name: 'hangar_action', args: { id: 'sessions', arg: '  a  ' } });
    expect(socket.messages).toContainEqual({ type: 'tool_result', call: 9, ok: false, text: '' });
    expect(socket.messages).toContainEqual({ type: 'tool_result', call: 10, ok: true, text: '' });
    expect(handlers.switchTo).toHaveBeenCalledWith({ server: 'peer', baseUrl: 'https://peer.test', name: 'other' });
    expect(handlers.runAction).toHaveBeenCalledWith('sessions', 'a');
    call.setMode('planejar'); call.setScreen({ server: 'peer', name: 'other' });
    expect(socket.messages).toContainEqual({ type: 'mode', mode: 'planejar' });
    expect(socket.messages).toContainEqual({ type: 'screen', screen: { server: 'peer', name: 'other' } });
    expect(fetch).not.toHaveBeenCalled();
  });

  it('native start tardio é seguido de stop antes do start da chamada seguinte', async () => {
    const entered = deferred<void>(), pending = deferred<void>();
    rtc.nativeStart.mockImplementationOnce(() => { entered.resolve(); return pending.promise; });
    const { call } = makeCall();
    const starting = call.start(server, null);
    await entered.promise;
    const stopping = call.stop();
    const next = call.start(server, null);
    const order: string[] = [];
    rtc.nativeStop.mockImplementation(async () => { order.push('stop'); });
    rtc.nativeStart.mockImplementation(async () => { order.push('start'); });
    pending.resolve();
    await Promise.all([starting, stopping, next]);
    expect(order).toEqual(['stop', 'start']);
    expect(Socket.all).toHaveLength(1);
    expect(Socket.all[0].close).not.toHaveBeenCalled();
  });

  it('reativar depois do ditado reaplica áudio nativo antes de ligar o track', async () => {
    const { call, pc } = await openCall(); pc.connect();
    const stream = await rtc.getUserMedia.mock.results[0].value;
    call.setMuted(true);
    const restored = deferred<void>(), entered = deferred<void>();
    rtc.nativeStart.mockImplementationOnce(() => { entered.resolve(); return restored.promise; });
    call.setMuted(false);
    await entered.promise;
    call.setMuted(false);
    expect(stream.track.enabled).toBe(false);
    restored.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(stream.track.enabled).toBe(true);
    expect(rtc.nativeStart).toHaveBeenCalledTimes(2);
  });

  it('restauração atrasada ou rejeitada não muda nem derruba a chamada posterior', async () => {
    const { call, pc, handlers } = await openCall(); pc.connect(); call.setMuted(true);
    const restore = deferred<void>(), entered = deferred<void>();
    rtc.nativeStart.mockImplementationOnce(() => { entered.resolve(); return restore.promise; });
    call.setMuted(false); await entered.promise;
    const next = call.start(server, null);
    restore.reject(new Error('old-audio'));
    await next;
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(Socket.all.at(-1)!.close).not.toHaveBeenCalled();
  });

  it('Encerrar na notificação usa o desligamento normal, sem erro de interrupção', async () => {
    const { socket, handlers } = await openCall();
    for (const listener of rtc.listeners) listener({ reason: 'stop' });
    await vi.advanceTimersByTimeAsync(0);
    expect(socket.messages).toContainEqual({ type: 'stop' });
    expect(handlers.phase).toHaveBeenLastCalledWith('closed');
    expect(handlers.failed).not.toHaveBeenCalled();
    expect(rtc.nativeStop).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['notifications_permission', 'notifications_permission'],
    ['microphone_permission', 'microphone'],
    ['app_not_visible', 'foreground_required'],
    ['audio_interrupted', 'audio_interrupted'],
  ])('preserva a recusa nativa %s como %s', async (nativeCode, shownCode) => {
    rtc.nativeStart.mockRejectedValueOnce(Object.assign(new Error('native-private-detail'), { code: nativeCode }));
    const { call, handlers } = makeCall();
    await call.start(server, null);
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith(shownCode, undefined);
    expect(Socket.all).toHaveLength(0);
  });

  it('recusa nativa desconhecida vira failed com detalhe estável', async () => {
    rtc.nativeStart.mockRejectedValueOnce(Object.assign(new Error('native-private-detail'), { code: 'unexpected_native_code' }));
    const { call, handlers } = makeCall();
    await call.start(server, null);
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('failed', 'native_audio_start_failed');
  });

  it('a restauração também conserva o código da recusa nativa e encerra a própria chamada', async () => {
    const { call, socket, pc, handlers } = await openCall(); pc.connect(); call.setMuted(true);
    rtc.nativeStart.mockRejectedValueOnce(Object.assign(new Error('blocked_channel'), { code: 'notifications_permission' }));
    call.setMuted(false);
    await vi.advanceTimersByTimeAsync(0);
    expect(handlers.failed).toHaveBeenCalledExactlyOnceWith('notifications_permission', undefined);
    expect(socket.messages).toContainEqual({ type: 'stop' });
  });

  it('interrupção fatal de mídia encerra só a própria chamada anexada', async () => {
    const { socket, handlers } = await openCall();
    for (const listener of rtc.listeners) listener({ reason: 'focus_lost' });
    expect(socket.messages).toContainEqual({ type: 'stop' });
    expect(handlers.failed).toHaveBeenCalledWith('audio_interrupted', 'focus_lost');
  });
});
