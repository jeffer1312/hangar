// Uma chamada de voz por app. A voz roda num servidor; os ids de servidor do PWA não são os do
// peers.json dele, então tela e troca de sessão passam pelo mapa por host montado de /api/peers.
import { baseOf, type Server } from '@hangar/core';
import { listServers } from './auth';
import { LiveVoiceCall, type LiveVoiceHandlers, type LiveVoiceScreen, type LiveVoiceState } from './liveVoice';
import { listarPeers, type PeerView } from './peers';

export type LiveVoicePhase = 'idle' | 'connecting' | 'live' | 'closed';
export type LiveVoiceTarget = { server: Server; name: string };
type Navigate = (target: LiveVoiceTarget) => boolean;

// Bloqueio de tela derruba o socket; passado isso, a chamada no servidor já encerrou.
const REOPEN_WITHIN = 2 * 60_000;

let phase = $state<LiveVoicePhase>('idle');
let voiceState = $state<LiveVoiceState | null>(null);
let error = $state<{ code: string; detail?: string } | null>(null);
let muted = $state(false);
let open = $state(false);
let levels = $state({ input: 0, output: 0 });

let call: LiveVoiceCall | null = null;
let voiceServer: Server | null = null;
let peers: PeerView[] = [];
let screen: LiveVoiceTarget | null = null;
let navigate: Navigate | null = null;
let lostAt: number | null = null;
let attempt = 0;

// ponytail: casa por hostname (sem porta); duas máquinas no mesmo host em portas diferentes se confundem.
function hostOf(url: string | null | undefined): string | null {
  try { return url ? new URL(url).hostname : null; } catch { return null; }
}

function sameHost(server: Server, ...urls: (string | null | undefined)[]): boolean {
  const own = [server.baseUrl, baseOf(server)].map(hostOf);
  return urls.some(url => { const host = hostOf(url); return host !== null && own.includes(host); });
}

function toVoice(target: LiveVoiceTarget | null): LiveVoiceScreen {
  if (!target || !voiceServer) return null;
  const s = target.server;
  if (s.id === voiceServer.id || sameHost(s, voiceServer.baseUrl, baseOf(voiceServer))) return { server: '', name: target.name };
  const peer = peers.find(p => sameHost(s, p.base_url));
  return peer ? { server: peer.id, name: target.name } : null;
}

function fromVoice(server: string, baseUrl: string | null): Server | null {
  if (!voiceServer) return null;
  if (!server) return voiceServer;
  const peer = peers.find(p => p.id === server);
  return listServers().find(s => sameHost(s, peer?.base_url, baseUrl)) ?? null;
}

const handlers: LiveVoiceHandlers = {
  phase(p) {
    phase = p;
    if (p === 'live') error = null;
    if (p === 'closed') lostAt = null;
  },
  state(s) { voiceState = s; },
  failed(code, detail) {
    error = { code, detail };
    phase = 'closed';
    lostAt = code === 'connection_lost' ? Date.now() : null;
  },
  taken() {
    error = { code: 'taken' };
    phase = 'closed';
    lostAt = null;
  },
  switchTo(target) {
    const server = fromVoice(target.server, target.baseUrl);
    return !!server && !!navigate && navigate({ server, name: target.name });
  },
  levels(v) { levels = v; },
};

function reopenIfLost() {
  if (document.visibilityState !== 'visible' || lostAt === null || !voiceServer) return;
  const recent = Date.now() - lostAt < REOPEN_WITHIN;
  lostAt = null;
  if (recent) void start(voiceServer);
}

function ensureCall(): LiveVoiceCall {
  if (call) return call;
  const audio = document.createElement('audio');
  document.body.appendChild(audio);
  document.addEventListener('visibilitychange', reopenIfLost);
  return call = new LiveVoiceCall(audio, handlers);
}

async function start(server: Server) {
  const mine = ++attempt;
  const current = ensureCall();
  voiceServer = server;
  peers = [];
  lostAt = null;
  error = null;
  voiceState = null;
  open = true;
  phase = 'connecting';
  try { peers = await listarPeers(server); }
  // Sem a lista, só a tela e a troca no próprio servidor da voz casam.
  catch (e) { console.warn('voz: peers do servidor da voz indisponíveis', e); }
  if (mine !== attempt) return;
  await current.start(server, toVoice(screen));
}

function stop() {
  attempt++;
  lostAt = null;
  if (call) call.stop();
  else phase = 'idle';
}

export const liveVoiceStore = {
  get phase() { return phase; },
  get state() { return voiceState; },
  get error() { return error; },
  get muted() { return muted; },
  get levels() { return levels; },
  get open() { return open; },
  set open(value: boolean) { open = value; },
  start,
  stop,
  toggleMute() {
    muted = !muted;
    call?.setMuted(muted);
  },
  setMode(mode: 'direto' | 'planejar') { call?.setMode(mode); },
  /** A sessão aberta no PWA; vai à voz já traduzida para os ids do servidor dela. */
  setScreen(next: LiveVoiceTarget | null) {
    screen = next;
    call?.setScreen(toVoice(next));
  },
  /** O App registra quem abre a sessão; devolve o cancelamento. */
  registerNavigator(fn: Navigate): () => void {
    navigate = fn;
    return () => { if (navigate === fn) navigate = null; };
  },
};
