import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CodexVoiceCall } from './codexVoice';
import { codexVoiceUrlForServer } from '@hangar/core';

const server = { id: 'remote', label: 'Remote', baseUrl: 'https://remote.test', token: 'secret' };

class Peer {
  static last: Peer;
  connectionState = 'new';
  ontrack: ((event: { streams: MediaStream[] }) => void) | null = null;
  onconnectionstatechange: (() => void) | null = null;
  addTrack = vi.fn();
  createDataChannel = vi.fn();
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'v=0\r\n' }));
  setLocalDescription = vi.fn(async () => {});
  setRemoteDescription = vi.fn(async () => {});
  close = vi.fn();
  constructor() { Peer.last = this; }
}

class Socket {
  static OPEN = 1;
  static last: Socket;
  readyState = 1;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => Promise<void>) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  send = vi.fn((_data: string) => {});
  close = vi.fn();
  constructor(public url: string) { Socket.last = this; }
}

function stream() {
  const track = { enabled: true, stop: vi.fn() };
  return { track, media: { getTracks: () => [track], getAudioTracks: () => [track] } as unknown as MediaStream };
}

function call() {
  const audio = { pause: vi.fn(), play: vi.fn(async () => {}), srcObject: null } as unknown as HTMLAudioElement;
  const changed = vi.fn();
  return { voice: new CodexVoiceCall(audio, changed, vi.fn()), changed };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal('RTCPeerConnection', Peer);
  vi.stubGlobal('WebSocket', Socket);
  vi.stubGlobal('location', { origin: 'https://local.test' });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('voz Codex', () => {
  it('libera microfone que chegou depois de encerrar', async () => {
    let resolve!: (media: MediaStream) => void;
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia: () => new Promise<MediaStream>(r => { resolve = r; }) } });
    const { voice, changed } = call();
    const pending = voice.start(server, 'sess', '');
    voice.stop();
    const { media, track } = stream();
    resolve(media);
    await pending;
    expect(track.stop).toHaveBeenCalledOnce();
    expect(changed).toHaveBeenLastCalledWith('idle');
  });

  it('negocia no servidor correto e libera todos os recursos no erro remoto', async () => {
    const { media, track } = stream();
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia: async () => media } });
    const { voice, changed } = call();
    await voice.start(server, 'sess / 2', 'coral');
    const ws = Socket.last, pc = Peer.last;
    expect(ws.url).toBe('wss://remote.test/api/sessions/sess%20%2F%202/codex/voice?token=secret');
    expect(track.enabled).toBe(false);
    await ws.onmessage!({ data: JSON.stringify({ type: 'ready', voices: ['coral'] }) });
    expect(JSON.parse(ws.send.mock.calls[0][0])).toEqual({ type: 'start', sdp: 'v=0\r\n', voice: 'coral' });
    await ws.onmessage!({ data: JSON.stringify({ method: 'thread/realtime/sdp', params: { sdp: 'answer' } }) });
    expect(pc.setRemoteDescription).toHaveBeenCalledWith({ type: 'answer', sdp: 'answer' });
    pc.connectionState = 'connected'; pc.onconnectionstatechange!();
    expect(track.enabled).toBe(true);
    voice.setMuted(true); expect(track.enabled).toBe(false);
    await ws.onmessage!({ data: JSON.stringify({ method: 'thread/realtime/error', params: { message: 'voice unavailable' } }) });
    expect(changed).toHaveBeenLastCalledWith('error', 'failed', 'voice unavailable');
    expect(track.stop).toHaveBeenCalledOnce();
    expect(pc.close).toHaveBeenCalledOnce();
    expect(ws.close).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('encerra conexão sem resposta e ignora callbacks antigos', async () => {
    const { media, track } = stream();
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia: async () => media } });
    const { voice, changed } = call();
    await voice.start(server, 'sess', '');
    const stale = Socket.last.onmessage!;
    await vi.advanceTimersByTimeAsync(45000);
    expect(track.stop).toHaveBeenCalledOnce();
    expect(changed).toHaveBeenLastCalledWith('error', 'timeout', undefined);
    await stale({ data: JSON.stringify({ type: 'ready', voices: [] }) });
    expect(Socket.last.send).not.toHaveBeenCalled();
  });

  it('expõe quando o serviço encerra ou a configuração é desligada', async () => {
    const first = stream();
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia: async () => first.media } });
    let result = call();
    await result.voice.start(server, 'sess', '');
    await Socket.last.onmessage!({ data: JSON.stringify({ method: 'thread/realtime/closed', params: {} }) });
    expect(result.changed).toHaveBeenLastCalledWith('error', 'closed', undefined);

    const second = stream();
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia: async () => second.media } });
    result = call();
    await result.voice.start(server, 'sess', '');
    await Socket.last.onmessage!({ data: JSON.stringify({ type: 'error', code: 'disabled' }) });
    expect(result.changed).toHaveBeenLastCalledWith('error', 'disabled', undefined);
  });

  it('resolve mesma origem sem depender do servidor ativo', () => {
    expect(codexVoiceUrlForServer({ ...server, baseUrl: '' }, 'x', 'https://local.test')).toBe(
      'wss://local.test/api/sessions/x/codex/voice?token=secret');
  });

  it('mede entrada e saída separadamente e zera ao silenciar ou encerrar', async () => {
    const input = stream(), output = stream();
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia: async () => input.media } });
    const closed = vi.fn(async () => {});
    let amplitude = 0;
    vi.stubGlobal('AudioContext', class {
      state = 'running'; close = closed; resume = async () => {};
      createMediaStreamSource = () => ({ connect: vi.fn(), disconnect: vi.fn() });
      createAnalyser = () => {
        const value = ++amplitude * 8;
        return { fftSize: 256, disconnect: vi.fn(), getByteTimeDomainData: (bytes: Uint8Array) => bytes.fill(128 + value) };
      };
    });
    const levels = vi.fn();
    const audio = { pause: vi.fn(), play: vi.fn(async () => {}), srcObject: null } as unknown as HTMLAudioElement;
    const voice = new CodexVoiceCall(audio, vi.fn(), vi.fn(), levels);
    await voice.start(server, 'sess', 'sol');
    Peer.last.connectionState = 'connected'; Peer.last.onconnectionstatechange!();
    Peer.last.ontrack!({ streams: [output.media] });
    await vi.advanceTimersByTimeAsync(60);
    expect(levels).toHaveBeenLastCalledWith({ input: 0.32, output: 0.62, available: true });
    voice.setMuted(true); await vi.advanceTimersByTimeAsync(60);
    expect(levels).toHaveBeenLastCalledWith({ input: 0, output: 0.62, available: true });
    voice.stop();
    expect(closed).toHaveBeenCalledOnce();
    expect(levels).toHaveBeenLastCalledWith({ input: 0, output: 0, available: false });
    expect(vi.getTimerCount()).toBe(0);
  });
});
