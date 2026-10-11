// Uma chamada de voz por app. A voz roda num servidor; os ids de servidor do PWA não são os do
// peers.json dele, então tela e troca de sessão passam pelo mapa por host montado de /api/peers.
import { untrack } from 'svelte';
import { baseOf, probeServerResponse, type Server } from '@hangar/core';
import { listServers } from './auth';
import { LiveVoiceCall, type LiveVoiceHandlers, type LiveVoiceScreen, type LiveVoiceState } from './liveVoice';
import { listarPeers, type PeerView } from './peers';
import * as m from '../paraglide/messages';
import { TELAS_CONFIG } from './configRoute';
import { settledSpeaker, speaker, voiceStatus, type Speaker, type VoiceStatus } from './liveVoiceStatus';

export type LiveVoicePhase = 'idle' | 'connecting' | 'live' | 'closed';
export type LiveVoiceTarget = { server: Server; name: string };
type Navigate = (target: LiveVoiceTarget) => boolean;

type ModePair = { model: string | null; effort: string; tier: string | null };
export interface LiveVoiceSettings {
  voice: string | null;
  codex_account: string;
  organizer: { direct: ModePair; plan: ModePair };
}
/** `GET /api/voice/settings`: a trava do beta, o Codex no servidor e as escolhas gravadas. */
export interface LiveVoiceSettingsReply {
  enabled: boolean;
  codex: boolean;
  jev: boolean;
  voices: string[];
  settings: LiveVoiceSettings;
  call: { active: boolean; client: string | null };
}
type SettingsEntry = { reply: LiveVoiceSettingsReply | null; error: string | null; loading: boolean };

// Bloqueio de tela derruba o socket; passado isso, a chamada no servidor já encerrou.
const REOPEN_WITHIN = 2 * 60_000;

let phase = $state<LiveVoicePhase>('idle');
let voiceState = $state<LiveVoiceState | null>(null);
let error = $state<{ code: string; detail?: string } | null>(null);
let muted = $state(false);
let open = $state(false);
let levels = $state({ input: 0, output: 0 });
let shownSpeaker = $state<Speaker>('idle');
let speakerSince = 0;
let activeServer = $state<Server | null>(null);
let shownServer = $state<Server | null>(null);
let settings = $state<Record<string, SettingsEntry>>({});

let call: LiveVoiceCall | null = null;
let voiceServer: Server | null = null;
let peers: PeerView[] = [];
let screen: LiveVoiceTarget | null = null;
let navigate: Navigate | null = null;
/** `true` = feito; texto = por que não (vai para a voz contar ao usuário). */
export type VoiceActionRun = (arg: string | null) => true | string;
// Cada tela registra as ações que só ela executa; o catálogo é fixo porque vai no hello da chamada.
const actionRuns = new Map<string, VoiceActionRun>();
const ACTION_IDS = ['session-list', 'settings', 'settings-back', 'new-session', 'voice-panel', 'voice-panel-close',
  'costs', 'usage', 'report-back', 'terminal', 'git', 'activity', 'panel-close'] as const;
export type VoiceActionId = (typeof ACTION_IDS)[number];

function actionCatalog() {
  const texts: Record<VoiceActionId, [string, string]> = {
    'session-list': [m.live_voice_action_list_label(), m.live_voice_action_list_description()],
    settings: [m.live_voice_action_settings_label(), m.live_voice_action_settings_description({ sections: TELAS_CONFIG.join(', ') })],
    'settings-back': [m.live_voice_action_settings_back_label(), m.live_voice_action_settings_back_description()],
    'new-session': [m.live_voice_action_new_session_label(), m.live_voice_action_new_session_description()],
    'voice-panel': [m.live_voice_action_voice_panel_label(), m.live_voice_action_voice_panel_description()],
    'voice-panel-close': [m.live_voice_action_voice_panel_close_label(), m.live_voice_action_voice_panel_close_description()],
    costs: [m.live_voice_action_costs_label(), m.live_voice_action_costs_description()],
    usage: [m.live_voice_action_usage_label(), m.live_voice_action_usage_description()],
    'report-back': [m.live_voice_action_report_back_label(), m.live_voice_action_report_back_description()],
    terminal: [m.live_voice_action_terminal_label(), m.live_voice_action_terminal_description()],
    git: [m.live_voice_action_git_label(), m.live_voice_action_git_description()],
    activity: [m.live_voice_action_activity_label(), m.live_voice_action_activity_description()],
    'panel-close': [m.live_voice_action_panel_close_label(), m.live_voice_action_panel_close_description()],
  };
  return ACTION_IDS.map(id => ({ id, label: texts[id][0], description: texts[id][1] }));
}
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
    voiceState = null;
    phase = 'closed';
    lostAt = code === 'connection_lost' ? Date.now() : null;
  },
  taken() {
    error = { code: 'taken' };
    voiceState = null;
    phase = 'closed';
    lostAt = null;
  },
  switchTo(target) {
    const server = fromVoice(target.server, target.baseUrl);
    return !!server && !!navigate && navigate({ server, name: target.name });
  },
  levels(v) {
    levels = v;
    [shownSpeaker, speakerSince] = settledSpeaker(shownSpeaker, speakerSince, speaker(v.input, v.output, muted), Date.now());
  },
  actions: actionCatalog,
  runAction(id, arg) {
    if (id === 'voice-panel' || id === 'voice-panel-close') { open = id === 'voice-panel'; return true; }
    const run = actionRuns.get(id);
    if (run) return run(arg);
    return (ACTION_IDS as readonly string[]).includes(id) ? m.live_voice_action_unavailable() : m.live_voice_action_unknown();
  },
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
  shownServer = server;
  peers = [];
  lostAt = null;
  error = null;
  voiceState = null;
  // Quem falava na chamada anterior não vale para esta.
  levels = { input: 0, output: 0 };
  shownSpeaker = 'idle';
  speakerSince = 0;
  open = true;
  phase = 'connecting';
  // A chamada não espera a lista: a tela de outra máquina só casa quando ela chega.
  listarPeers(server).then(list => {
    if (mine !== attempt) return;
    peers = list;
    current.setScreen(toVoice(screen));
  }, e => console.warn('live voice: peers of the voice server unavailable', e));
  await current.start(server, toVoice(screen));
}

function stop() {
  attempt++;
  lostAt = null;
  voiceState = null;
  if (call) call.stop();
  else phase = 'idle';
}

async function settingsRequest(server: Server, init?: RequestInit): Promise<{ settings: LiveVoiceSettings } & Partial<LiveVoiceSettingsReply>> {
  const res = await probeServerResponse(server, '/api/voice/settings', init);
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw Object.assign(new Error(body?.error_code ?? `http_${res.status}`), { code: body?.error_code ?? 'failed' });
  return body;
}

/** Uma leitura por servidor; `force` relê (o painel abre com o retrato atual). */
async function loadSettings(server: Server, force = false) {
  // Chamado de dentro de efeitos: ler sem rastrear, senão a própria gravação relança a leitura.
  const current = untrack(() => settings[server.id]);
  if (current && (current.loading || !force && current.reply)) return;
  settings[server.id] = { reply: current?.reply ?? null, error: null, loading: true };
  try {
    const reply = await settingsRequest(server) as LiveVoiceSettingsReply;
    settings[server.id] = { reply, error: null, loading: false };
  } catch (e) {
    console.warn('live voice: settings unreadable', e);
    settings[server.id] = { reply: current?.reply ?? null, error: e instanceof Error ? e.message : 'failed', loading: false };
  }
}

// A trava da voz muda nas opções do Codex; sem isto o botão só reflete a troca depois de reler os ajustes.
if (typeof window !== 'undefined') {
  window.addEventListener('hangar:codex-voice-config', (event) => {
    const detail = (event as CustomEvent<{ serverId: string | null; enabled: boolean } | undefined>).detail;
    if (!detail) return;
    const id = detail.serverId ?? activeServer?.id;
    if (!id) return;
    const entry = settings[id];
    if (entry?.reply) settings[id] = { ...entry, reply: { ...entry.reply, enabled: detail.enabled === true } };
  });
}

/** Grava o objeto inteiro; o erro sobe com o `error_code` do servidor. */
async function saveSettings(server: Server, next: LiveVoiceSettings) {
  const { settings: saved } = await settingsRequest(server, { method: 'PUT', body: JSON.stringify(next) });
  const reply = settings[server.id]?.reply;
  if (reply) settings[server.id] = { reply: { ...reply, settings: saved }, error: null, loading: false };
}

export const liveVoiceStore = {
  get phase() { return phase; },
  get state() { return voiceState; },
  get error() { return error; },
  get muted() { return muted; },
  get levels() { return levels; },
  /** Rótulo da chamada: quem fala, mudo, ou o que a voz está fazendo (pensando, pesquisando…). */
  get status(): VoiceStatus { return voiceStatus(phase === 'live', shownSpeaker, muted, voiceState?.activity); },
  get open() { return open; },
  set open(value: boolean) { open = value; },
  /** Servidor ativo do PWA, publicado pelo App a cada rota (o `getActiveId` não é reativo). */
  get activeServer() { return activeServer; },
  set activeServer(value: Server | null) { activeServer = value; },
  /** Servidor da última chamada aberta; os ajustes do painel são dele. */
  get server() { return shownServer; },
  settingsOf(server: Server | null): SettingsEntry | null { return server ? settings[server.id] ?? null : null; },
  loadSettings,
  saveSettings,
  start,
  stop,
  /** A pessoa já viu o erro no painel: o botão volta ao normal. */
  dismissError() { error = null; },
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
  /** A tela montada registra as ações que ela executa; devolve o cancelamento (só tira as que ainda são dela). */
  registerActions(runs: Partial<Record<VoiceActionId, VoiceActionRun>>): () => void {
    const mine = Object.entries(runs) as [VoiceActionId, VoiceActionRun][];
    for (const [id, run] of mine) actionRuns.set(id, run);
    return () => { for (const [id, run] of mine) if (actionRuns.get(id) === run) actionRuns.delete(id); };
  },
};
