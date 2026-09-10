import { codexVoiceUrlForServer, type Server } from '@hangar/core';

export type VoiceState = 'idle' | 'connecting' | 'connected' | 'error';
export type VoiceFailure = 'unavailable' | 'busy' | 'disabled' | 'closed' | 'failed' | 'microphone' | 'timeout' | 'playback';
export interface VoiceLevels { input: number; output: number; available: boolean }

export class CodexVoiceCall {
  private pc: RTCPeerConnection | null = null;
  private ws: WebSocket | null = null;
  private media: MediaStream | null = null;
  private generation = 0;
  private deadline: ReturnType<typeof setTimeout> | undefined;
  private heartbeat: ReturnType<typeof setInterval> | undefined;
  private lastPong = 0;
  private muted = false;
  private meterContext: AudioContext | null = null;
  private meterTimer: ReturnType<typeof setInterval> | undefined;
  private meters: Partial<Record<'input' | 'output', { source: MediaStreamAudioSourceNode; analyser: AnalyserNode; samples: Uint8Array<ArrayBuffer> }>> = {};

  constructor(private audio: HTMLAudioElement, private changed: (state: VoiceState, error?: VoiceFailure, detail?: string) => void,
    private voices: (values: string[]) => void,
    private levels: (values: VoiceLevels) => void = () => {},
    private draft: (text: string | null) => void = () => {}) {}

  private meter(side: 'input' | 'output', stream: MediaStream) {
    try {
      if (!this.meterContext) {
        this.meterContext = new AudioContext();
        const context = this.meterContext;
        void context.resume().catch(() => {
          if (this.meterContext === context) this.levels({ input: 0, output: 0, available: false });
        });
        this.meterTimer = setInterval(() => {
          const read = (key: 'input' | 'output') => {
            const meter = this.meters[key];
            if (!meter || this.pc?.connectionState !== 'connected' || key === 'input' && this.muted) return 0;
            meter.analyser.getByteTimeDomainData(meter.samples);
            const mean = meter.samples.reduce((sum, value) => sum + ((value - 128) / 128) ** 2, 0) / meter.samples.length;
            return Math.round(Math.min(1, Math.sqrt(mean) * 5) * 50) / 50;
          };
          this.levels({ input: read('input'), output: read('output'), available: this.meterContext?.state === 'running' && !!this.meters.input });
        }, 60);
      }
      this.meters[side]?.source.disconnect();
      this.meters[side]?.analyser.disconnect();
      const source = this.meterContext.createMediaStreamSource(stream);
      const analyser = this.meterContext.createAnalyser();
      analyser.fftSize = 256;
      source.connect(analyser);
      this.meters[side] = { source, analyser, samples: new Uint8Array(analyser.fftSize) };
    } catch {
      this.levels({ input: 0, output: 0, available: false });
    }
  }

  async start(server: Server, name: string, voice: string) {
    this.stop();
    const generation = this.generation;
    const current = () => generation === this.generation;
    this.changed('connecting');
    this.deadline = setTimeout(() => this.fail('timeout'), 45000);
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
        this.audio.play().catch(() => { if (current()) this.fail('playback'); });
      };
      pc.onconnectionstatechange = () => {
        if (!current()) return;
        if (pc.connectionState === 'connected') {
          clearTimeout(this.deadline);
          this.setMuted(this.muted);
          this.changed('connected');
        } else if (['failed', 'disconnected', 'closed'].includes(pc.connectionState)) this.fail('failed');
      };
      pc.createDataChannel('oai-events');
      const offer = await pc.createOffer();
      if (!current()) return;
      await pc.setLocalDescription(offer);
      if (!current()) return;
      const ws = this.ws = new WebSocket(codexVoiceUrlForServer(server, name, location.origin));
      const startHeartbeat = () => {
        this.lastPong = Date.now();
        this.heartbeat = setInterval(() => {
          if (!current()) return;
          if (Date.now() - this.lastPong > 30000) { this.fail('timeout'); return; }
          if (ws.readyState === WebSocket.OPEN) ws.send('ping');
        }, 10000);
      };
      ws.onmessage = async event => {
        if (!current()) return;
        try {
          const data = JSON.parse(event.data);
          if (data.type === 'pong') this.lastPong = Date.now();
          if (data.type === 'draft') this.draft(typeof data.request === 'string' ? data.request : null);
          if (data.type === 'submitted') this.draft(null);
          if (data.type === 'ready') {
            this.voices(Array.isArray(data.voices) ? data.voices.filter((v: unknown) => typeof v === 'string') : []);
            ws.send(JSON.stringify({ type: 'start', sdp: offer.sdp, voice: voice || null }));
            startHeartbeat();
          }
          if (data.method === 'thread/realtime/sdp') await pc.setRemoteDescription({ type: 'answer', sdp: data.params.sdp });
          if (data.type === 'error') this.fail(data.code === 'busy' ? 'busy' : data.code === 'disabled' ? 'disabled' : data.code === 'unavailable' ? 'unavailable' : 'failed');
          if (data.method === 'thread/realtime/error') this.fail('failed', data.params?.message);
          if (data.method === 'thread/realtime/closed') this.fail('closed');
        } catch { if (current()) this.fail('failed'); }
      };
      ws.onerror = ws.onclose = () => { if (current()) this.fail('failed'); };
    } catch {
      if (current()) this.fail(this.media ? 'failed' : 'microphone');
    }
  }

  setMuted(muted: boolean) {
    this.muted = muted;
    this.media?.getAudioTracks().forEach(track => { track.enabled = !muted && this.pc?.connectionState === 'connected'; });
  }

  private fail(error: VoiceFailure, detail?: string) {
    this.stop();
    this.changed('error', error, detail);
  }

  stop() {
    this.generation++;
    clearTimeout(this.deadline);
    clearInterval(this.heartbeat);
    clearInterval(this.meterTimer);
    for (const meter of Object.values(this.meters)) { meter.source.disconnect(); meter.analyser.disconnect(); }
    this.meters = {};
    void this.meterContext?.close().catch(() => {});
    this.meterContext = null;
    this.levels({ input: 0, output: 0, available: false });
    this.draft(null);
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
    this.changed('idle');
  }
}
