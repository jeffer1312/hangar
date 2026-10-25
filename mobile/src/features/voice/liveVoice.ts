import { PermissionsAndroid, Platform } from 'react-native';
import { mediaDevices, permissions as mediaPermissions, RTCPeerConnection, type MediaStream } from 'react-native-webrtc';
import {
  LIVE_VOICE_CAPS, liveVoiceBarLevel, liveVoiceRms, liveVoiceUrlForServer, parseLiveVoiceMessage,
  registrarDiag, type LiveVoiceAudioSample, type LiveVoiceClientMessage, type LiveVoiceHandlers,
  type LiveVoiceMode, type LiveVoiceScreen, type Server,
} from '@hangar/core';
import LiveVoiceAudio from '../../../modules/live-voice-audio';
import * as m from '../../paraglide/messages';

const PING_MS = 10_000;
const TIMEOUT_MS = 30_000;
const METER_MS = 100;
const METER_TIMEOUT_MS = 10_000;
const OPENING_MS = 60_000;
const DISCONNECT_GRACE_MS = 5_000;

type Attempt = {
  closed: boolean;
  attached: boolean;
  live: boolean;
  /** A conexão WebRTC atual chegou a conectar; `live` é da chamada, que sobrevive à troca dela. */
  pcLive: boolean;
  answered: boolean;
  terminal?: 'failed' | 'taken' | 'closed';
  media?: MediaStream;
  pc?: RTCPeerConnection;
  channel?: ReturnType<RTCPeerConnection['createDataChannel']>;
  ws?: WebSocket;
  listener?: { remove(): void };
  clock?: { remove(): void };
  createdAt: number;
  lastPong: number;
  lastPing: number;
  lastMeter: number;
  lastInput: number;
  reading: boolean;
  restoring: boolean;
  audioRevision: number;
  samples: Map<string, LiveVoiceAudioSample>;
  cleanup?: Promise<void>;
  openingDeadline?: number;
  openingTimer?: ReturnType<typeof setTimeout>;
  cancelStart(): void;
  disconnectedAt?: number;
};

// O áudio nativo é do aparelho: o stop antigo precisa acabar antes do start novo, até entre instâncias.
let audioTail: Promise<void> = Promise.resolve();
let audioOwner: Attempt | null = null;
function serializeAudio(operation: () => Promise<void>): Promise<void> {
  const result = audioTail.then(operation);
  // Quem pediu a operação recebe a rejeição; só a fila precisa continuar depois dela.
  audioTail = result.catch(() => {});
  return result;
}
function detail(error: unknown): string | undefined {
  const message = typeof error === 'object' && error !== null && 'message' in error ? error.message : error;
  if (typeof message !== 'string' || !message) return undefined;
  return /token(?:=|%3d)/i.test(message) ? 'token=[redacted]' : message;
}
function releaseMedia(media: MediaStream): void {
  for (const track of media.getTracks()) { track.onended = null; track.stop(); }
  media.release();
}

/** Transporte da chamada; não conhece stores, rotas de tela nem o envio de mensagens às sessões. */
export class LiveVoiceCall {
  private attempt: Attempt | null = null;
  private muted = false;
  private screen: LiveVoiceScreen = null;

  constructor(private handlers: LiveVoiceHandlers) {}

  private current(call: Attempt): boolean {
    if (this.attempt !== call || call.closed) return false;
    if (!call.live && call.openingDeadline !== undefined && Date.now() >= call.openingDeadline) {
      this.fail(call, 'opening_timeout', undefined, true);
      return false;
    }
    return true;
  }

  async start(server: Server, screen: LiveVoiceScreen, openingDeadline = Date.now() + OPENING_MS): Promise<void> {
    const previous = this.attempt;
    const cleanup = previous ? this.cleanup(previous) : Promise.resolve();
    let cancelStart!: () => void;
    const cancelled = new Promise<void>(resolve => { cancelStart = resolve; });
    const call: Attempt = {
      closed: false, attached: false, live: false, pcLive: false, answered: false, reading: false, restoring: false,
      createdAt: 0, lastPong: 0, lastPing: 0, lastMeter: 0, lastInput: 0, audioRevision: 0, samples: new Map(),
      openingDeadline, cancelStart,
    };
    this.attempt = call;
    this.screen = screen;
    this.handlers.phase('connecting');
    call.openingTimer = setTimeout(() => { this.current(call); }, Math.max(0, openingDeadline - Date.now()));
    await Promise.race([this.open(call, server, cleanup), cancelled]);
  }

  private async open(call: Attempt, server: Server, cleanup: Promise<void>): Promise<void> {
    let failure = 'failed';
    try {
      // A falha anterior já foi publicada; ela não pertence à tentativa nova.
      await cleanup.catch(() => {});
      if (!this.current(call)) return;
      if (typeof __DEV__ !== 'undefined' && !__DEV__) {
        // A entrada principal do WebRTC não exporta o controle dos logs.
        const { default: Logger } = require('react-native-webrtc/lib/commonjs/Logger') as typeof import('react-native-webrtc/lib/typescript/Logger');
        Logger.enable(`${Logger.ROOT_PREFIX}:*,-${Logger.ROOT_PREFIX}:*:DEBUG`);
      }
      if (server.invite || server.removed) { this.fail(call, 'owner_only'); return; }
      // A origem vazia é deliberada no nativo: nunca usar a máquina ATIVA como reserva para A.
      let url: string;
      try {
        url = liveVoiceUrlForServer(server, '');
        const parsed = new URL(url);
        if ((parsed.protocol !== 'ws:' && parsed.protocol !== 'wss:') || !parsed.hostname) throw new Error();
      } catch { this.fail(call, 'failed', 'invalid_server_url'); return; }
      // A decisão nos diálogos é da pessoa; o prazo volta a contar só na captura.
      clearTimeout(call.openingTimer);
      call.openingDeadline = undefined;
      failure = 'notifications_permission';
      if (Platform.OS === 'android' && Number(Platform.Version) >= 33) {
        const permission = await PermissionsAndroid.request(PermissionsAndroid.PERMISSIONS.POST_NOTIFICATIONS);
        if (!this.current(call)) return;
        if (permission !== PermissionsAndroid.RESULTS.GRANTED) { this.fail(call, failure); return; }
      }
      failure = 'failed';
      if (Platform.OS === 'android' && Number(Platform.Version) >= 31) {
        // Sem permissão Bluetooth o nativo usa outra saída; só uma falha do pedido interrompe.
        await PermissionsAndroid.request(PermissionsAndroid.PERMISSIONS.BLUETOOTH_CONNECT);
        if (!this.current(call)) return;
      }
      failure = 'microphone';
      const microphone = await mediaPermissions.request({ name: 'microphone' });
      if (!this.current(call)) return;
      if (microphone !== true) { this.fail(call, failure); return; }
      call.openingDeadline = Date.now() + OPENING_MS;
      call.openingTimer = setTimeout(() => { this.current(call); }, OPENING_MS);
      const media = await mediaDevices.getUserMedia({ audio: true, video: false });
      if (!this.current(call)) { releaseMedia(media); return; }
      call.media = media;
      if (!media.getAudioTracks().length) throw new Error('audio_track_missing');
      for (const track of media.getAudioTracks()) {
        track.enabled = false;
        track.onended = () => { if (this.current(call)) this.fail(call, 'microphone', undefined, true); };
      }
      failure = 'failed';
      call.listener = LiveVoiceAudio.addListener('interruption', event => {
        if (!this.current(call)) return;
        if (event.reason === 'stop') void this.stop().catch(() => {});
        else this.fail(call, 'audio_interrupted', event.reason, true);
      });
      // O relógio nativo continua no fundo; timers JS do RN param quando a Activity pausa.
      call.clock = LiveVoiceAudio.addListener('tick', () => this.tick(call));
      try { await this.startAudio(call); }
      catch (error) { this.failAudio(call, error); return; }
      if (!this.current(call)) return;
      const sdp = await this.connect(call, media);
      if (!sdp) return;
      let ws: WebSocket;
      const NativeWebSocket = WebSocket as typeof WebSocket & { new (url: string, protocols: string[], options: { headers: { Origin: string } }): WebSocket };
      try { ws = call.ws = new NativeWebSocket(url, [], { headers: { Origin: new URL(url.replace(/^ws/, 'http')).origin } }); }
      catch { this.fail(call, 'failed', 'websocket_start_failed', true); return; }
      call.createdAt = call.lastPong = call.lastPing = Date.now();
      ws.onopen = () => {
        if (!this.current(call)) return;
        try {
          if (!this.send(call, { type: 'hello', client: 'mobile', screen: this.screen, caps: LIVE_VOICE_CAPS, actions: this.handlers.actions() })) return;
          call.attached = true;
          this.send(call, { type: 'offer', sdp });
        } catch (error) { this.fail(call, 'failed', detail(error), true); }
      };
      ws.onmessage = event => {
        if (!this.current(call)) return;
        if (typeof event.data !== 'string') { this.fail(call, 'invalid_response'); return; }
        void this.receive(call, event.data).catch(error => {
          if (this.current(call)) this.fail(call, 'failed', detail(error), true);
        });
      };
      ws.onerror = () => { if (this.current(call)) this.fail(call, 'connection_lost'); };
      ws.onclose = () => { if (this.current(call)) this.fail(call, 'connection_lost'); };
    } catch (error) {
      if (this.current(call)) this.fail(call, failure, detail(error), true);
    }
  }

  /** Conexão WebRTC com o microfone e o áudio nativo já abertos; `null` = a tentativa mudou no meio. */
  private async connect(call: Attempt, media: MediaStream): Promise<string | null> {
    const pc = call.pc = new RTCPeerConnection();
    const mine = () => this.current(call) && call.pc === pc;
    call.pcLive = call.answered = false;
    call.disconnectedAt = undefined;
    for (const track of media.getAudioTracks()) pc.addTrack(track, media);
    pc.onconnectionstatechange = () => {
      if (!mine()) return;
      if (pc.connectionState === 'connected') {
        call.disconnectedAt = undefined;
        this.updateTracks(call);
        if (call.pcLive) return;
        if (!this.send(call, { type: 'live' })) return;
        call.pcLive = call.live = true;
        clearTimeout(call.openingTimer);
        call.lastInput = call.lastMeter = Date.now();
        this.handlers.phase('live');
      } else if (pc.connectionState === 'disconnected') {
        call.disconnectedAt ??= Date.now();
      } else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
        this.dropped(call, pc);
      }
    };
    // O Realtime exige esta linha na oferta, mesmo sem tráfego do cliente no canal.
    call.channel = pc.createDataChannel('oai-events');
    const offer = await pc.createOffer();
    if (!mine()) return null;
    if (!offer.sdp?.trim()) throw new Error('empty_offer');
    await pc.setLocalDescription(offer);
    return mine() ? offer.sdp : null;
  }

  /** A OpenAI encerra a sessão de áudio por tempo; o servidor guarda a conversa e espera outra oferta pelo mesmo socket. */
  private dropped(call: Attempt, pc: RTCPeerConnection): void {
    if (!this.current(call) || call.pc !== pc) return;
    if (!call.pcLive || !call.media) { this.fail(call, 'connection_lost'); return; }
    pc.onconnectionstatechange = null;
    try { call.channel?.close(); pc.close(); } catch { /* a conexão velha já caiu */ }
    this.updateTracks(call);
    this.handlers.phase('connecting');
    void this.connect(call, call.media).then(sdp => { if (sdp) this.send(call, { type: 'offer', sdp }); },
      error => { if (this.current(call)) this.fail(call, 'failed', detail(error), true); });
  }

  private startAudio(call: Attempt, revision?: number): Promise<void> {
    return serializeAudio(async () => {
      if (!this.current(call) || revision !== undefined && call.audioRevision !== revision) return;
      audioOwner = call;
      await LiveVoiceAudio.start({ title: m.live_voice_open(), body: m.native_live_voice_notification_body(), stopLabel: m.live_voice_stop() });
    });
  }

  private failAudio(call: Attempt, error: unknown): void {
    const code = typeof error === 'object' && error !== null && 'code' in error ? error.code : undefined;
    if (code === 'microphone_permission') this.fail(call, 'microphone', undefined, true);
    else if (code === 'app_not_visible') this.fail(call, 'foreground_required', undefined, true);
    else if (code === 'notifications_permission' || code === 'audio_interrupted') this.fail(call, code, undefined, true);
    else this.fail(call, 'failed', 'native_audio_start_failed', true);
  }

  private send(call: Attempt, message: LiveVoiceClientMessage): boolean {
    if (!this.current(call) || call.ws?.readyState !== WebSocket.OPEN) return false;
    try { call.ws.send(JSON.stringify(message)); return true; }
    catch (error) { this.fail(call, 'connection_lost', detail(error)); return false; }
  }

  private async receive(call: Attempt, raw: string): Promise<void> {
    if (!this.current(call)) return;
    const message = parseLiveVoiceMessage(raw);
    if (!message) { this.fail(call, 'invalid_response'); return; }
    switch (message.type) {
      case 'answer':
        if (call.answered || !call.pc) { this.fail(call, 'invalid_response'); return; }
        call.answered = true;
        try { await call.pc.setRemoteDescription({ type: 'answer', sdp: message.sdp }); }
        catch (error) { if (this.current(call)) this.fail(call, 'failed', detail(error), true); }
        break;
      case 'state': this.handlers.state(message.state); break;
      case 'tool': this.tool(call, message.call, message.name, message.args); break;
      case 'pong': call.lastPong = Date.now(); break;
      case 'taken':
        call.terminal = 'taken';
        void this.cleanup(call);
        this.handlers.taken();
        break;
      case 'error': this.fail(call, message.code, message.detail); break;
      case 'closed':
        call.terminal = 'closed';
        void this.cleanup(call);
        this.handlers.phase('closed');
        break;
    }
  }

  private tool(call: Attempt, id: number, name: string, value: unknown): void {
    const args = typeof value === 'object' && value !== null && !Array.isArray(value) ? value : {};
    let ok = false, text = '';
    try {
      if (name === 'hangar_actions') {
        text = this.handlers.actions().map(action => `- ${action.id}: ${action.label}. ${action.description}`).join('\n');
        ok = true;
      } else if (name === 'hangar_action') {
        const action = 'id' in args && typeof args.id === 'string' ? args.id : '';
        const arg = 'arg' in args && typeof args.arg === 'string' && args.arg.trim() ? args.arg.trim() : null;
        const result = this.handlers.runAction(action, arg);
        ok = result === true;
        text = result === true ? '' : result;
      } else if (name === 'switch_session') {
        ok = this.handlers.switchTo({
          server: 'server' in args && typeof args.server === 'string' ? args.server : '',
          baseUrl: 'base_url' in args && typeof args.base_url === 'string' ? args.base_url : null,
          name: 'name' in args && typeof args.name === 'string' ? args.name : '',
        });
      }
    } catch (error) { text = detail(error) ?? ''; }
    // Desconhecida ou sem detalhe: o servidor traduz a recusa no idioma da chamada.
    this.send(call, { type: 'tool_result', call: id, ok, text });
  }

  setScreen(screen: LiveVoiceScreen): void {
    this.screen = screen;
    if (this.attempt) this.send(this.attempt, { type: 'screen', screen });
  }

  setMuted(muted: boolean): void {
    const wasMuted = this.muted;
    this.muted = muted;
    const call = this.attempt;
    if (!call || !this.current(call)) return;
    if (wasMuted === muted) { this.updateTracks(call); return; }
    const revision = ++call.audioRevision;
    call.restoring = false;
    if (!muted && wasMuted && call.media) {
      // O ditado Expo pode ter trocado a categoria e liberado a rota de áudio.
      call.restoring = true;
      this.updateTracks(call);
      void this.startAudio(call, revision).then(() => {
        if (!this.current(call) || call.audioRevision !== revision) return;
        call.restoring = false;
        call.lastInput = Date.now();
        this.updateTracks(call);
      }, error => { if (this.current(call) && call.audioRevision === revision) this.failAudio(call, error); });
    } else this.updateTracks(call);
  }

  private updateTracks(call: Attempt): void {
    const enabled = this.current(call) && !this.muted && !call.restoring
      && (call.pc?.connectionState === 'connected' || call.live && call.pc?.connectionState === 'disconnected');
    for (const track of call.media?.getAudioTracks() ?? []) track.enabled = enabled;
  }

  setMode(mode: LiveVoiceMode): void {
    if (this.attempt) this.send(this.attempt, { type: 'mode', mode });
  }

  async stop(): Promise<void> {
    const call = this.attempt;
    if (!call) return;
    if (this.current(call)) {
      if (call.attached) this.send(call, { type: 'stop' });
      // send pode ter encerrado por erro de rede: não apaga essa causa.
      if (!this.current(call)) return call.cleanup;
      call.terminal = 'closed';
      const cleanup = this.cleanup(call);
      this.handlers.phase('closed');
      await cleanup;
    } else await call.cleanup;
  }

  /** Rede/troca de dono deixam a conversa do organizador viva pelos 120 s do servidor. */
  private fail(call: Attempt, code: string, message?: string, fatalMedia = false): void {
    if (this.attempt !== call || call.closed) return;
    call.terminal = 'failed';
    if (fatalMedia && call.attached && call.ws?.readyState === WebSocket.OPEN) {
      // Não passar por send: uma falha no stop não pode substituir a causa original.
      try { call.ws.send(JSON.stringify({ type: 'stop' })); }
      catch { registrarDiag({ evento: 'voice.stop_failed', nivel: 'erro', codigo: 'connection_lost' }); }
    }
    void this.cleanup(call);
    this.handlers.failed(code, detail(message));
  }

  private tick(call: Attempt): void {
    if (!this.current(call)) return;
    const now = Date.now();
    if (call.disconnectedAt !== undefined && now - call.disconnectedAt >= DISCONNECT_GRACE_MS) {
      if (call.pc) this.dropped(call, call.pc);
      else this.fail(call, 'connection_lost');
      return;
    }
    if (call.ws) {
      if (now - call.lastPong >= TIMEOUT_MS || !call.live && now - call.createdAt >= TIMEOUT_MS) {
        this.fail(call, 'connection_lost');
        return;
      }
      if (now - call.lastPing >= PING_MS && call.ws.readyState === WebSocket.OPEN) {
        call.lastPing = now;
        if (!this.send(call, { type: 'ping' })) return;
      }
    }
    if (call.live && now - call.lastMeter >= METER_MS) {
      call.lastMeter = now;
      this.meter(call);
    }
  }

  // O nível segura envios no servidor mesmo com a folha fechada; não é só animação.
  private meter(call: Attempt): void {
    if (!this.current(call) || call.pc?.connectionState !== 'connected' && call.pc?.connectionState !== 'disconnected') return;
    const now = Date.now();
    const silent = this.muted || call.restoring || !call.media?.getAudioTracks().some(track => track.enabled);
    // Um getStats pendurado não pode prender o último nível positivo depois de silenciar.
    if (silent) { if (!this.send(call, { type: 'level', input: 0 })) return; }
    else if (now - call.lastInput >= METER_TIMEOUT_MS) {
      this.fail(call, 'audio_levels_unavailable', 'getStats: audioLevel/totalAudioEnergy/totalSamplesDuration', true);
      return;
    }
    if (call.reading) return;
    call.reading = true;
    void this.readLevels(call).catch(() => {
      if (!this.current(call)) return;
      if (this.muted || call.restoring) this.handlers.levels({ input: 0, output: 0 });
      else this.fail(call, 'audio_levels_unavailable', 'getStats_failed', true);
    }).finally(() => { call.reading = false; });
  }

  private async readLevels(call: Attempt): Promise<void> {
    const reports = await call.pc!.getStats();
    if (!this.current(call) || call.pc?.connectionState !== 'connected' && call.pc?.connectionState !== 'disconnected') return;
    let input: number | null = null, output: number | null = null;
    let inputRank = 0, outputRank = 0;
    const samples = new Map<string, LiveVoiceAudioSample>();
    for (const [id, row] of reports) {
      if (typeof row !== 'object' || row === null || row.kind !== 'audio' && row.mediaType !== 'audio') continue;
      const sample: LiveVoiceAudioSample = {
        audioLevel: typeof row.audioLevel === 'number' ? row.audioLevel : undefined,
        totalAudioEnergy: typeof row.totalAudioEnergy === 'number' ? row.totalAudioEnergy : undefined,
        totalSamplesDuration: typeof row.totalSamplesDuration === 'number' ? row.totalSamplesDuration : undefined,
      };
      const key = String(id);
      samples.set(key, sample);
      const rms = liveVoiceRms(sample, call.samples.get(key));
      if (rms === null) continue;
      const incoming = row.type === 'inbound-rtp' ? 2 : row.type === 'track' && row.remoteSource === true ? 1 : 0;
      const outgoing = row.type === 'media-source' ? 3 : row.type === 'track' && row.remoteSource === false ? 2 : row.type === 'outbound-rtp' ? 1 : 0;
      if (outgoing > inputRank) { input = rms; inputRank = outgoing; }
      if (incoming > outputRank) { output = rms; outputRank = incoming; }
    }
    call.samples = samples;
    const now = Date.now();
    if (input !== null) call.lastInput = now;
    // A saída pode não ter inbound-rtp até a primeira fala; só a entrada decide o silêncio no servidor.
    // Desabilitado é um zero conhecido. Amostra ausente não autoriza o envio no servidor.
    const silent = this.muted || call.restoring || !call.media?.getAudioTracks().some(track => track.enabled);
    if (silent) input = 0;
    this.handlers.levels({ input: liveVoiceBarLevel(input ?? 0), output: liveVoiceBarLevel(output ?? 0) });
    if (!silent && input !== null) this.send(call, { type: 'level', input });
  }

  private cleanup(call: Attempt): Promise<void> {
    if (call.cleanup) return call.cleanup;
    call.closed = true;
    clearTimeout(call.openingTimer);
    call.cancelStart();
    const errors: unknown[] = [];
    const release = (operation: () => void) => { try { operation(); } catch (error) { errors.push(error); } };
    if (call.ws) {
      call.ws.onopen = call.ws.onmessage = call.ws.onerror = call.ws.onclose = null;
      release(() => call.ws!.close());
    }
    release(() => call.listener?.remove());
    release(() => call.clock?.remove());
    if (call.pc) {
      call.pc.onconnectionstatechange = null;
      release(() => call.channel?.close());
      release(() => call.pc!.close());
    }
    if (call.media) release(() => releaseMedia(call.media!));
    call.samples.clear();
    this.handlers.levels({ input: 0, output: 0 });
    call.cleanup = serializeAudio(async () => {
      if (audioOwner !== call) return;
      audioOwner = null;
      await LiveVoiceAudio.stop();
    }).catch(error => { errors.push(error); }).then(() => {
      if (!errors.length) return;
      throw Object.assign(new Error('audio_restore'), { code: 'audio_restore' });
    });
    // O resultado continua rejeitado para quem aguarda stop; fechamentos internos também têm dono para o erro.
    void call.cleanup.catch(() => {
      registrarDiag({ evento: 'voice.cleanup_failed', nivel: 'erro', codigo: 'audio_restore' });
      if (this.attempt === call && call.terminal !== 'failed' && call.terminal !== 'taken') this.handlers.failed('audio_restore', undefined);
    });
    return call.cleanup;
  }
}
