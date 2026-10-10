import { liveVoiceUrlForServer, type Server } from '@hangar/core';

/** Sessão na tela, nos ids do servidor da voz: `server` vazio = o próprio servidor da voz. */
export type LiveVoiceScreen = { server: string; name: string } | null;

type RateWindow = { used_percent: number; resets_at: number | null } | null;

/** Retrato que o servidor manda em `state` (`state_json` do controlador da voz). */
export interface LiveVoiceState {
  phase: string;
  mode: 'direct' | 'plan';
  activity: 'idle' | 'thinking' | 'searching' | 'working';
  draft: string | null;
  error: { code: string; text: string } | null;
  plan: { path: string; markdown: string } | null;
  effective: { model: string | null; effort: string | null; tier: string | null } | null;
  context: { used: number; window: number | null } | null;
  limits: { five_hour: RateWindow; seven_day: RateWindow };
  backstage: { kind: 'heard' | 'result' | 'answer'; text: string }[];
  thought: string;
  action: { kind: 'tool' | 'search' | 'command'; text: string } | null;
  followed: { server: string; name: string }[];
  server: string;
  client: string;
}

export interface LiveVoiceHandlers {
  phase(p: 'connecting' | 'live' | 'closed'): void;
  state(s: LiveVoiceState): void;
  failed(code: string, detail?: string): void;
  taken(): void;
  switchTo(target: { server: string; baseUrl: string | null; name: string }): boolean;
  levels(v: { input: number; output: number }): void;
}

export class LiveVoiceCall {
  private pc: RTCPeerConnection | null = null;
  private ws: WebSocket | null = null;
  private media: MediaStream | null = null;
  private generation = 0;
  private heartbeat: ReturnType<typeof setInterval> | undefined;
  private lastPong = 0;
  private muted = false;
  private screen: LiveVoiceScreen = null;
  private meterContext: AudioContext | null = null;
  private meterTimer: ReturnType<typeof setInterval> | undefined;
  private levelTimer: ReturnType<typeof setInterval> | undefined;
  private meters: Partial<Record<'input' | 'output', { source: MediaStreamAudioSourceNode; analyser: AnalyserNode; samples: Uint8Array<ArrayBuffer> }>> = {};

  constructor(private audio: HTMLAudioElement, private handlers: LiveVoiceHandlers) {}

  private send(msg: object) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg));
  }

  private rms(side: 'input' | 'output'): number {
    const meter = this.meters[side];
    if (!meter || this.pc?.connectionState !== 'connected' || side === 'input' && this.muted) return 0;
    meter.analyser.getByteTimeDomainData(meter.samples);
    return Math.sqrt(meter.samples.reduce((sum, value) => sum + ((value - 128) / 128) ** 2, 0) / meter.samples.length);
  }

  private meter(side: 'input' | 'output', stream: MediaStream) {
    try {
      if (!this.meterContext) {
        this.meterContext = new AudioContext();
        void this.meterContext.resume().catch(() => {});
        const bar = (key: 'input' | 'output') => Math.round(Math.min(1, this.rms(key) * 5) * 50) / 50;
        this.meterTimer = setInterval(() => this.handlers.levels({ input: bar('input'), output: bar('output') }), 60);
      }
      this.meters[side]?.source.disconnect();
      this.meters[side]?.analyser.disconnect();
      const source = this.meterContext.createMediaStreamSource(stream);
      const analyser = this.meterContext.createAnalyser();
      analyser.fftSize = 256;
      source.connect(analyser);
      this.meters[side] = { source, analyser, samples: new Uint8Array(analyser.fftSize) };
    } catch {
      this.handlers.levels({ input: 0, output: 0 });
    }
  }

  async start(server: Server, screen: LiveVoiceScreen) {
    this.teardown();
    const generation = this.generation;
    const current = () => generation === this.generation;
    this.screen = screen;
    this.handlers.phase('connecting');
    try {
      const media = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true } });
      if (!current()) { media.getTracks().forEach(track => track.stop()); return; }
      this.media = media;
      media.getTracks().forEach(track => { track.enabled = false; });
      const pc = this.pc = new RTCPeerConnection();
      media.getTracks().forEach(track => pc.addTrack(track, media));
      this.meter('input', media);
      pc.ontrack = event => {
        if (!current()) return;
        this.audio.srcObject = event.streams[0] ?? new MediaStream([event.track]);
        this.meter('output', this.audio.srcObject as MediaStream);
        this.audio.play().catch(() => { if (current()) this.end('playback'); });
      };
      pc.onconnectionstatechange = () => {
        if (!current()) return;
        if (pc.connectionState === 'connected') {
          this.setMuted(this.muted);
          this.send({ type: 'live' });
          this.handlers.phase('live');
          clearInterval(this.levelTimer);
          // RMS cru: o limiar de fala do servidor foi medido sem o ganho da barra.
          this.levelTimer = setInterval(() => { if (this.meters.input) this.send({ type: 'level', input: this.rms('input') }); }, 100);
        } else if (pc.connectionState === 'failed') this.fail('connection_lost');
      };
      // O Realtime exige a linha de dados na oferta, mesmo sem uso aqui.
      pc.createDataChannel('oai-events');
      const offer = await pc.createOffer();
      if (!current()) return;
      await pc.setLocalDescription(offer);
      if (!current()) return;
      const ws = this.ws = new WebSocket(liveVoiceUrlForServer(server, location.origin));
      // Também vale para a conexão que nunca abre: rede de VPN pendura em vez de recusar.
      this.lastPong = Date.now();
      this.heartbeat = setInterval(() => {
        if (Date.now() - this.lastPong > 30000) this.fail('connection_lost');
        else this.send({ type: 'ping' });
      }, 10000);
      ws.onopen = () => {
        this.send({ type: 'hello', client: 'pwa', screen: this.screen, caps: ['switch_session'] });
        this.send({ type: 'offer', sdp: offer.sdp });
      };
      ws.onmessage = event => { void this.receive(event.data, pc); };
      ws.onclose = () => this.fail('connection_lost');
    } catch {
      if (current()) this.end(this.media ? 'failed' : 'microphone');
    }
  }

  private async receive(raw: string, pc: RTCPeerConnection) {
    let msg;
    try { msg = JSON.parse(raw); } catch { console.warn('live voice: unreadable server message'); return; }
    switch (msg.type) {
      case 'answer':
        try { await pc.setRemoteDescription({ type: 'answer', sdp: msg.sdp }); }
        catch (e) { if (pc === this.pc) this.end('failed', e instanceof Error ? e.message : undefined); }
        break;
      case 'state': this.handlers.state(msg.state); break;
      case 'tool': this.tool(msg.call, msg.name, msg.args ?? {}); break;
      case 'pong': this.lastPong = Date.now(); break;
      case 'taken': this.teardown(); this.handlers.taken(); break;
      case 'error': this.fail(String(msg.code), msg.detail ?? undefined); break;
      case 'closed': this.teardown(); this.handlers.phase('closed'); break;
    }
  }

  private tool(call: number, name: string, args: Record<string, unknown>) {
    if (name !== 'switch_session') {
      // Recusa sem texto: o servidor fala a recusa no idioma da voz.
      this.send({ type: 'tool_result', call, ok: false, text: '' });
      return;
    }
    let ok = false;
    try {
      ok = this.handlers.switchTo({
        server: typeof args.server === 'string' ? args.server : '',
        baseUrl: typeof args.base_url === 'string' ? args.base_url : null,
        name: String(args.name ?? ''),
      });
    } catch (e) { console.warn('live voice: session switch failed', e); }
    this.send({ type: 'tool_result', call, ok, text: '' });
  }

  setScreen(screen: LiveVoiceScreen) {
    this.screen = screen;
    this.send({ type: 'screen', screen });
  }

  setMuted(muted: boolean) {
    this.muted = muted;
    this.media?.getAudioTracks().forEach(track => { track.enabled = !muted && this.pc?.connectionState === 'connected'; });
  }

  setMode(mode: 'direto' | 'planejar') {
    this.send({ type: 'mode', mode });
  }

  stop() {
    this.send({ type: 'stop' });
    this.teardown();
    this.handlers.phase('closed');
  }

  /** A chamada no servidor fica viva pelo prazo de passagem: outro aparelho, ou este de novo, assume. */
  private fail(code: string, detail?: string) {
    this.teardown();
    this.handlers.failed(code, detail);
  }

  /** Falha deste aparelho que não se resolve reconectando: encerra também no servidor. */
  private end(code: string, detail?: string) {
    this.send({ type: 'stop' });
    this.fail(code, detail);
  }

  private teardown() {
    this.generation++;
    clearInterval(this.heartbeat);
    clearInterval(this.meterTimer);
    clearInterval(this.levelTimer);
    for (const meter of Object.values(this.meters)) { meter.source.disconnect(); meter.analyser.disconnect(); }
    this.meters = {};
    void this.meterContext?.close().catch(() => {});
    this.meterContext = null;
    this.handlers.levels({ input: 0, output: 0 });
    if (this.ws) {
      this.ws.onopen = this.ws.onmessage = this.ws.onerror = this.ws.onclose = null;
      this.ws.close();
      this.ws = null;
    }
    this.media?.getTracks().forEach(track => track.stop());
    this.media = null;
    if (this.pc) {
      this.pc.ontrack = this.pc.onconnectionstatechange = null;
      this.pc.close();
      this.pc = null;
    }
    this.audio.pause();
    this.audio.srcObject = null;
  }
}
