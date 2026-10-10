import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LiveVoiceCall, type LiveVoiceHandlers } from './liveVoice';

const server = { id: 'remote', label: 'Remote', baseUrl: 'https://remote.test', token: 'secret' };

class FakePc {
  static last: FakePc;
  connectionState = 'new';
  remoteDescription: RTCSessionDescriptionInit | null = null;
  ontrack: ((event: { streams: MediaStream[] }) => void) | null = null;
  onconnectionstatechange: (() => void) | null = null;
  addTrack = vi.fn();
  createDataChannel = vi.fn();
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'v=0 offer' }));
  setLocalDescription = vi.fn(async () => {});
  setRemoteDescription = vi.fn(async (description: RTCSessionDescriptionInit) => { this.remoteDescription = description; });
  close = vi.fn();
  constructor() { FakePc.last = this; }
  connect() {
    this.connectionState = 'connected';
    this.onconnectionstatechange?.();
  }
}

class FakeWs {
  static OPEN = 1;
  static opened = 0;
  static last: FakeWs;
  readyState = 0;
  sent: any[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  send = vi.fn((data: string) => { this.sent.push(JSON.parse(data)); });
  close = vi.fn(() => { this.readyState = 3; });
  constructor(public url: string) { FakeWs.opened++; FakeWs.last = this; }
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  receive(obj: object) { this.onmessage?.({ data: JSON.stringify(obj) }); }
  closeFromServer() {
    this.readyState = 3;
    this.onclose?.();
  }
}

// Amostras fixas em 192: RMS cru 0,5, que a barra (×5) mostra cheia.
class FakeAudioContext {
  state = 'running';
  resume = async () => {};
  close = async () => {};
  createMediaStreamSource = () => ({ connect: vi.fn(), disconnect: vi.fn() });
  createAnalyser = () => ({ fftSize: 0, connect: vi.fn(), disconnect: vi.fn(), getByteTimeDomainData: (samples: Uint8Array) => samples.fill(192) });
}

function media() {
  const track = { enabled: true, stop: vi.fn() };
  return { getTracks: () => [track], getAudioTracks: () => [track] } as unknown as MediaStream;
}

async function started(handlers: Partial<LiveVoiceHandlers> = {}) {
  const audio = { pause: vi.fn(), play: vi.fn(async () => {}), srcObject: null } as unknown as HTMLAudioElement;
  const call = new LiveVoiceCall(audio, {
    phase: vi.fn(), state: vi.fn(), failed: vi.fn(), taken: vi.fn(), switchTo: vi.fn(() => false), levels: vi.fn(), ...handlers,
  });
  await call.start(server, { server: '', name: 'hangar' });
  const ws = FakeWs.last, pc = FakePc.last;
  ws.open();
  return { call, ws, pc };
}

beforeEach(() => {
  FakeWs.opened = 0;
  vi.stubGlobal('RTCPeerConnection', FakePc);
  vi.stubGlobal('WebSocket', FakeWs);
  vi.stubGlobal('location', { origin: 'https://local.test' });
  vi.stubGlobal('navigator', { mediaDevices: { getUserMedia: async () => media() } });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('LiveVoiceCall', () => {
  it('says hello with the PWA capability, sends the offer and applies the answer', async () => {
    const { call, ws, pc } = await started();
    expect(ws.url).toBe('wss://remote.test/api/voice?token=secret');
    expect(ws.sent[0]).toEqual({ type: 'hello', client: 'pwa', screen: { server: '', name: 'hangar' }, caps: ['switch_session'] });
    expect(ws.sent[1].type).toBe('offer');
    ws.receive({ type: 'answer', sdp: 'v=0 answer' });
    await vi.waitFor(() => expect(pc.remoteDescription?.sdp).toBe('v=0 answer'));
    call.stop();
  });

  it('turns a switch tool into navigation and answers the server', async () => {
    const switchTo = vi.fn(() => true);
    const { ws } = await started({ switchTo });
    ws.receive({ type: 'tool', call: 3, name: 'switch_session', args: { server: '', base_url: null, name: 'web' } });
    expect(switchTo).toHaveBeenCalledWith({ server: '', baseUrl: null, name: 'web' });
    expect(ws.sent.at(-1)).toEqual({ type: 'tool_result', call: 3, ok: true, text: '' });
  });

  it('refuses a screen tool it does not support', async () => {
    const { ws } = await started();
    ws.receive({ type: 'tool', call: 4, name: 'read_screen', args: {} });
    expect(ws.sent.at(-1)).toEqual({ type: 'tool_result', call: 4, ok: false, text: '' });
  });

  it('taken_does_not_reconnect', async () => {
    const taken = vi.fn();
    const { ws } = await started({ taken });
    ws.receive({ type: 'taken' });
    expect(taken).toHaveBeenCalledOnce();
    expect(FakeWs.opened).toBe(1);
  });

  it('unexpected_close_reports_connection_lost', async () => {
    const failed = vi.fn();
    const { ws } = await started({ failed });
    ws.closeFromServer();
    expect(failed).toHaveBeenCalledWith('connection_lost', undefined);
    expect(FakeWs.opened).toBe(1);
  });

  it('sends raw microphone level at most ten times a second', async () => {
    vi.useFakeTimers();
    const { ws, pc } = await started();
    pc.connect();
    vi.advanceTimersByTime(1000);
    expect(ws.sent.filter(m => m.type === 'level').length).toBeLessThanOrEqual(10);
    vi.useRealTimers();
  });

  it('goes live on connection and sends the raw RMS while the bar keeps the gain', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('AudioContext', FakeAudioContext);
    const levels = vi.fn();
    const { ws, pc } = await started({ levels });
    pc.connect();
    expect(ws.sent.at(-1)).toEqual({ type: 'live' });
    vi.advanceTimersByTime(100);
    expect(ws.sent.filter(m => m.type === 'level')).toEqual([{ type: 'level', input: 0.5 }]);
    expect(levels).toHaveBeenLastCalledWith({ input: 1, output: 0 });
  });

  it('a server error ends the call without reporting a lost connection', async () => {
    const failed = vi.fn();
    const { ws } = await started({ failed });
    ws.receive({ type: 'error', code: 'disabled', detail: null });
    ws.closeFromServer();
    expect(failed).toHaveBeenCalledTimes(1);
    expect(failed).toHaveBeenCalledWith('disabled', undefined);
  });
});
