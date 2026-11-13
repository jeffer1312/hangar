import { create } from 'zustand';
import {
  baseOf, loadLiveVoiceSettings, saveLiveVoiceSettings, listVoiceMemory, forgetVoiceMemory,
  listarCredenciais, modelOptionsForServer, probeServerResponse, registrarDiag, motivoDoServidor,
} from '@hangar/core';
import type { LiveVoiceAction, LiveVoiceHandlers, LiveVoiceScreen, LiveVoiceSettings, Server } from '@hangar/core';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';
import { haptic } from '../../ui/haptics';
import { toast } from '../../ui/Toast';
import { setLiveVoiceAudioActive, useVoiceAudioState } from './audioSession';
import type { LiveVoiceCall } from './liveVoice';
import type { LiveVoiceView } from './LiveVoiceSheet';

type SettingsView = LiveVoiceView['settings'];
type MemoryView = LiveVoiceView['memory'];
type SettingsEntry = { server: Server; view: SettingsView };
type Peer = { id: string; base_url: string; web_url?: string };
export type LiveVoiceRoute = { serverId: string; name: string; kind: 'chat' | 'session-panel' } | null;
export type LiveVoiceDestination =
  | { kind: 'session'; serverId: string; name: string; panel?: 'activity' | 'terminal' }
  | { kind: 'list' }
  | { kind: 'settings'; serverId: string; section: string | null };

type Navigation = (destination: LiveVoiceDestination) => boolean | void;
interface LiveVoiceStore {
  open: boolean;
  server: Server | null;
  view: LiveVoiceView;
  settingsByServer: Record<string, SettingsEntry>;
  foreground: boolean;
  route: LiveVoiceRoute;
  chatControls: { serverId: string; name: string } | null;
  openFor(serverId: string, name: string): void;
  selectServer(serverId: string): void;
  dismiss(): void;
  start(): Promise<void>;
  stop(): Promise<void>;
  retry(): Promise<void>;
  toggleMute(): void;
  toggleSpeechPause(): void;
  setMode(mode: 'direto' | 'planejar'): void;
  loadSettings(server: Server, force?: boolean): Promise<void>;
  saveSettings(next: LiveVoiceSettings): Promise<void>;
  openSettings(): void;
  refreshMemory(): Promise<void>;
  forgetMemory(id: string): Promise<void>;
  registerNavigation(navigate: Navigation): () => void;
  registerChat(context: { serverId: string; name: string; terminal: boolean; controls?: boolean }): () => void;
  setRoute(route: LiveVoiceRoute): void;
  setForeground(active: boolean): void;
  reconcileServers(): void;
}

const emptySettings = (): SettingsView => ({ loading: false, reply: null, error: null, saving: false,
  saveError: null, accounts: [], models: [], modelsError: null });
const emptyMemory = (): MemoryView => ({ loading: false, entries: [], error: null, forgetError: null });
const emptyView = (): LiveVoiceView => ({ phase: 'idle', serverLabel: '', state: null, error: null, muted: false,
  dictating: false, levels: { input: 0, output: 0 }, liveSince: null, settings: emptySettings(), memory: emptyMemory() });
const snapshot = (server: Server): Server => ({ ...server, ...(server.lan ? { lan: { ...server.lan } } : {}) });
const sameServer = (a: Server | null | undefined, b: Server) => a?.id === b.id && a.baseUrl === b.baseUrl && a.token === b.token;
const callable = (server: Server) => !server.invite && !server.inviteEnded && !server.removed && !server.disabled && !!server.token;
const inCall = (view: LiveVoiceView) => view.phase === 'connecting' || view.phase === 'live';
const errorCode = (error: unknown) => typeof error === 'object' && error !== null && 'code' in error && typeof error.code === 'string' ? error.code : 'connection_lost';
const errorText = (error: unknown) => motivoDoServidor(error) ?? (error instanceof Error && 'status' in error && typeof error.status === 'number' ? error.message : m.native_network_error());
const CONFIG_SECTIONS = new Set(['scheduled-tasks', 'geral', 'aparencia', 'usage-log', 'sobre', 'maquinas', 'sync', 'shared-config',
  'contas', 'orchestration', 'harnesses', 'voice', 'jev', 'notifications', 'shortcuts', 'attachments', 'worktrees', 'advanced']);

function normalizedUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) return null;
    return `${url.origin}${url.pathname.replace(/\/+$/, '')}`;
  } catch { return null; }
}
function addresses(server: Server): string[] {
  return [...new Set([server.baseUrl, baseOf(server), server.lan?.url].map(normalizedUrl).filter((url): url is string => url !== null))];
}
function peerAddresses(peer: Peer): string[] {
  return [peer.base_url, peer.web_url].map(normalizedUrl).filter((url): url is string => url !== null);
}

export const useLiveVoice = create<LiveVoiceStore>((set, get) => {
  let generation = 0;
  let call: LiveVoiceCall | null = null;
  let stopping: Promise<void> = Promise.resolve();
  let opening: { token: number; deadline?: number; timer: ReturnType<typeof setTimeout>; cancel(): void } | null = null;
  let userMuted = false;
  let settingsOpen = false;
  let selectedServerId: string | null = null;
  let navigator: { navigate: Navigation } | null = null;
  let chat: { serverId: string; name: string; terminal: boolean } | null = null;
  let peers: Peer[] = [];
  let peersGeneration = 0;
  let settingsSequence = 0;
  let modelsSequence = 0;
  let detailsSequence = 0;
  let memorySequence = 0;
  const settingsRequests = new Map<string, { sequence: number; promise: Promise<void> }>();

  const updateView = (patch: Partial<LiveVoiceView>) => set(state => ({ view: { ...state.view, ...patch } }));
  function settingsFor(server: Server): SettingsView {
    const entry = get().settingsByServer[server.id];
    return sameServer(entry?.server, server) ? entry.view : emptySettings();
  }
  function updateSettings(server: Server, update: (view: SettingsView) => SettingsView): void {
    const view = update(settingsFor(server));
    set(state => ({ settingsByServer: { ...state.settingsByServer, [server.id]: { server, view } },
      ...(sameServer(state.server, server) ? { view: { ...state.view, settings: view } } : {}) }));
  }
  function currentServer(): Server | null {
    const server = get().server;
    if (!server) return null;
    const saved = useServers.getState().servers.find(s => sameServer(s, server));
    return saved && callable(saved) ? server : null;
  }
  function ownMatch(urls: string[]): Server | null {
    const matches = useServers.getState().servers.filter(s => callable(s) && addresses(s).some(url => urls.includes(url)));
    return matches.length === 1 ? matches[0] : null;
  }
  function peerFor(server: Server): Peer | null {
    const urls = addresses(server);
    const matches = peers.filter(peer => peerAddresses(peer).some(url => urls.includes(url)));
    // Uma entrada local duplicada também torna a identidade incerta.
    return matches.length === 1 && ownMatch(peerAddresses(matches[0]))?.id === server.id ? matches[0] : null;
  }
  function screen(): LiveVoiceScreen {
    const state = get();
    if (!state.foreground || !state.route || !state.server) return null;
    const target = useServers.getState().servers.find(s => s.id === state.route!.serverId && callable(s));
    if (!target) return null;
    if (sameServer(target, state.server)) return { server: '', name: state.route.name };
    const peer = peerFor(target);
    return peer ? { server: peer.id, name: state.route.name } : null;
  }
  function publishScreen(): void { call?.setScreen(screen()); }
  async function loadPeers(server: Server): Promise<void> {
    const sequence = ++peersGeneration;
    peers = [];
    try {
      const response = await probeServerResponse(server, '/api/peers');
      if (!response.ok) throw new Error(`http_${response.status}`);
      const body: unknown = await response.json();
      if (!Array.isArray(body) || !body.every(p => p && typeof p === 'object' && typeof p.id === 'string' && typeof p.base_url === 'string'
        && (p.web_url === undefined || p.web_url === null || typeof p.web_url === 'string'))) throw new Error('invalid_response');
      if (sequence !== peersGeneration || !sameServer(get().server, server)) return;
      peers = body.map(p => ({ id: p.id, base_url: p.base_url, ...(typeof p.web_url === 'string' ? { web_url: p.web_url } : {}) }));
      publishScreen();
    } catch (error) {
      // Sem mapa conferido, ferramentas de troca para outras máquinas são recusadas.
      if (sequence === peersGeneration && sameServer(get().server, server)) {
        peers = [];
        const code = error instanceof Error && /^(http_\d{3}|invalid_response)$/.test(error.message) ? error.message : errorCode(error);
        registrarDiag({ evento: 'voz.peers_falhou', nivel: 'aviso', codigo: code });
      }
    }
  }
  function stopLocal(instance: LiveVoiceCall | null): Promise<void> {
    if (!instance) return stopping;
    const token = generation;
    // O transporte cancela a tentativa antes do primeiro await do stop.
    stopping = Promise.all([stopping, instance.stop()]).then(() => {}).catch(() => {
      if (generation === token) updateView({ error: get().view.error ?? { code: 'audio_restore' } });
    });
    return stopping;
  }
  function cancelOpening(): void {
    if (!opening) return;
    clearTimeout(opening.timer);
    opening.cancel();
    opening = null;
  }
  function end(error: LiveVoiceView['error'], stopRemote = true): Promise<void> {
    generation++;
    cancelOpening();
    const instance = call;
    call = null;
    setLiveVoiceAudioActive(false);
    updateView({ phase: 'closed', error: error ?? get().view.error, levels: { input: 0, output: 0 }, liveSince: null, dictating: false });
    return stopRemote ? stopLocal(instance) : Promise.resolve();
  }
  function actions(): LiveVoiceAction[] {
    return [
      { id: 'session-list', label: m.live_voice_action_list_label(), description: m.live_voice_action_list_description() },
      { id: 'settings', label: m.live_voice_action_settings_label(), description: m.live_voice_action_settings_description({ sections: [...CONFIG_SECTIONS].join(', ') }) },
      { id: 'voice-panel', label: m.live_voice_action_voice_panel_label(), description: m.live_voice_action_voice_panel_description() },
      { id: 'voice-panel-close', label: m.live_voice_action_voice_panel_close_label(), description: m.live_voice_action_voice_panel_close_description() },
      { id: 'activity', label: m.live_voice_action_activity_label(), description: m.live_voice_action_activity_description() },
      ...(get().route?.kind === 'chat' && chat?.terminal && chat.serverId === get().route?.serverId && chat.name === get().route?.name
        ? [{ id: 'terminal', label: m.live_voice_action_terminal_label(), description: m.live_voice_action_terminal_description() }] : []),
    ];
  }
  function runAction(id: string, arg: string | null): true | string {
    if (!get().foreground || !navigator || !currentServer()) return m.live_voice_action_unavailable();
    if (id === 'voice-panel') { set({ open: true }); return true; }
    if (id === 'voice-panel-close') {
      if (!get().open) return m.live_voice_action_not_open();
      get().dismiss(); return true;
    }
    if (id === 'session-list') return navigator.navigate({ kind: 'list' }) === false ? m.live_voice_action_unavailable() : true;
    if (id === 'settings') {
      const section = arg?.trim() || null;
      if (section && !CONFIG_SECTIONS.has(section)) return m.live_voice_action_bad_section({ section });
      const route = get().route;
      const target = route ? useServers.getState().servers.find(s => s.id === route.serverId && callable(s)) : currentServer();
      if (!target) return m.live_voice_action_unavailable();
      return navigator.navigate({ kind: 'settings', serverId: target.id, section }) === false ? m.live_voice_action_unavailable() : true;
    }
    if (id !== 'activity' && id !== 'terminal') return m.live_voice_action_unknown();
    const route = get().route;
    if (route?.kind !== 'chat' || !chat || chat.serverId !== route.serverId || chat.name !== route.name) return m.live_voice_action_unavailable();
    const target = useServers.getState().servers.find(s => s.id === route.serverId && callable(s));
    if (!target) return m.live_voice_action_unavailable();
    if (id === 'terminal' && !chat.terminal) return m.live_voice_action_no_terminal();
    return navigator.navigate({ kind: 'session', serverId: target.id, name: route.name, panel: id }) === false ? m.live_voice_action_unavailable() : true;
  }
  function handlers(token: number): LiveVoiceHandlers {
    const valid = () => generation === token;
    return {
      phase(phase) {
        if (!valid()) return;
        if (phase === 'closed') { void end(null); return; }
        if (phase === 'live') cancelOpening();
        updateView({ phase, ...(phase === 'live' ? { liveSince: Date.now() } : {}) });
      },
      state(state) {
        if (!valid()) return;
        const previous = get().view.state;
        const previousMemory = previous?.memory;
        if (state.stalled === true && previous?.stalled !== true) haptic.stalled();
        updateView({ state });
        if (settingsOpen && state.memory !== previousMemory) void get().refreshMemory();
      },
      failed(code, detail) { if (valid()) void end({ code, ...(detail ? { detail } : {}) }); },
      taken() { if (valid()) void end({ code: 'taken' }, false); },
      levels(levels) { if (valid()) updateView({ levels }); },
      switchTo(target) {
        if (!valid() || !get().foreground || !navigator || !currentServer() || !target.name) return false;
        const pinned = get().server!;
        let server: Server | null;
        if (target.server === '') {
          server = pinned;
          if (target.baseUrl && !addresses(pinned).includes(normalizedUrl(target.baseUrl) ?? '')) return false;
          if (ownMatch(addresses(pinned))?.id !== pinned.id) return false;
        } else {
          const matches = peers.filter(p => p.id === target.server);
          if (matches.length !== 1) return false;
          const urls = peerAddresses(matches[0]);
          if (target.baseUrl && !urls.includes(normalizedUrl(target.baseUrl) ?? '')) return false;
          server = ownMatch(urls);
        }
        if (!server) return false;
        return navigator.navigate({ kind: 'session', serverId: server.id, name: target.name }) !== false;
      },
      actions: () => valid() ? actions() : [],
      runAction: (id, arg) => valid() ? runAction(id, arg) : m.live_voice_action_unavailable(),
    };
  }
  async function loadModels(server: Server, account: string): Promise<void> {
    const sequence = ++modelsSequence;
    updateSettings(server, view => ({ ...view, models: [], modelsError: null }));
    try {
      const result = await modelOptionsForServer(server, 'codex', null, null, account);
      if (sequence !== modelsSequence || settingsFor(server).reply?.settings.codex_account !== account) return;
      updateSettings(server, view => ({ ...view, models: result.models, modelsError: null }));
    } catch (error) {
      if (sequence === modelsSequence && settingsFor(server).reply?.settings.codex_account === account)
        updateSettings(server, view => ({ ...view, models: [], modelsError: errorText(error) }));
    }
  }
  async function loadDetails(server: Server): Promise<void> {
    const sequence = ++detailsSequence;
    updateSettings(server, view => ({ ...view, error: null, accounts: [] }));
    try {
      const accounts = await listarCredenciais(server);
      if (sequence === detailsSequence && sameServer(get().server, server)) updateSettings(server, view => ({ ...view, accounts: accounts.filter(account => account.tipo === 'codex') }));
    } catch (error) {
      if (sequence === detailsSequence && sameServer(get().server, server)) updateSettings(server, view => ({ ...view, error: errorText(error) }));
    }
  }

  function availableServer(preferredId: string | null): Server | undefined {
    const { servers, activeId } = useServers.getState();
    const own = servers.filter(callable);
    return own.find(s => s.id === preferredId) ?? own.find(s => s.id === activeId) ?? own[0];
  }
  function captureServer(saved: Server): void {
    const server = snapshot(saved);
    generation++;
    settingsOpen = false;
    memorySequence++;
    detailsSequence++;
    modelsSequence++;
    peersGeneration++;
    peers = [];
    set({ server, view: { ...emptyView(), serverLabel: server.label, settings: settingsFor(server) } });
    void get().loadSettings(server, true);
  }

  useVoiceAudioState.subscribe((state, previous) => {
    if (state.dictations === previous.dictations || !inCall(get().view)) return;
    const muted = userMuted || state.dictations > 0;
    updateView({ muted, dictating: state.dictations > 0 });
    call?.setMuted(muted);
  });

  return {
    open: false, server: null, view: emptyView(), settingsByServer: {}, foreground: true, route: null, chatControls: null,
    openFor(serverId, _name) {
      if (inCall(get().view) || get().view.error && currentServer()) { set({ open: true }); return; }
      const saved = availableServer(selectedServerId ?? serverId);
      if (!saved) return;
      captureServer(saved);
      set({ open: true });
    },
    selectServer(serverId) {
      if (inCall(get().view)) return;
      const saved = useServers.getState().servers.find(s => s.id === serverId && callable(s));
      if (!saved) return;
      selectedServerId = saved.id;
      captureServer(saved);
    },
    dismiss() {
      settingsOpen = false;
      set({ open: false });
      if (!inCall(get().view)) updateView({ phase: 'idle', error: null, state: null });
    },
    async start() {
      if (inCall(get().view)) { set({ open: true }); return; }
      const server = currentServer();
      if (!server) { updateView({ error: { code: 'server_removed', detail: m.chat_servidor_removido() } }); return; }
      if (!get().foreground) { updateView({ error: { code: 'foreground_required' } }); return; }
      if (useVoiceAudioState.getState().dictations > 0) { updateView({ error: { code: 'dictation_active' } }); return; }
      const deadline = Date.now() + 60_000;
      const token = ++generation;
      cancelOpening();
      let cancel!: () => void;
      const cancelled = new Promise<void>(resolve => { cancel = resolve; });
      const valid = () => {
        if (generation !== token) return false;
        if (opening?.deadline === undefined || Date.now() < opening.deadline) return true;
        void end({ code: 'opening_timeout' });
        return false;
      };
      opening = { token, deadline, cancel, timer: setTimeout(valid, Math.max(0, deadline - Date.now())) };
      userMuted = false;
      // A trava vem antes de qualquer import, permissão ou leitura de rede.
      updateView({ phase: 'connecting', error: null, state: null, liveSince: null, muted: false, dictating: false, speechPaused: false });
      setLiveVoiceAudioActive(true);
      void loadPeers(server);
      const start = async () => {
        try {
          await stopping;
          if (!valid()) return;
          const { LiveVoiceCall } = await import('./liveVoice');
          if (!valid()) return;
          if (!currentServer()) { void end({ code: 'server_removed', detail: m.chat_servidor_removido() }); return; }
          const instance = new LiveVoiceCall(handlers(token));
          call = instance;
          updateView({ speechPauseAvailable: instance.canPauseSpeech() });
          instance.setMuted(userMuted || useVoiceAudioState.getState().dictations > 0);
          // O transporte assume a preparação e arma outro prazo após os diálogos de permissão.
          if (opening?.token === token) {
            clearTimeout(opening.timer);
            opening.deadline = undefined;
          }
          await instance.start(server, screen(), deadline);
          if (generation !== token) return;
          instance.setMuted(userMuted || useVoiceAudioState.getState().dictations > 0);
        } catch (error) {
          if (generation === token) void end({ code: errorCode(error) });
        }
      };
      await Promise.race([start(), cancelled]);
    },
    async stop() {
      updateView({ error: null });
      await end(null);
    },
    async retry() {
      const server = currentServer();
      if (!server || inCall(get().view)) return;
      if (get().view.error) { await get().start(); return; }
      await get().loadSettings(server, true);
      if (settingsOpen && sameServer(get().server, server)) { void loadDetails(server); void get().refreshMemory(); }
    },
    toggleMute() {
      if (!inCall(get().view) || useVoiceAudioState.getState().dictations > 0) return;
      userMuted = !userMuted;
      updateView({ muted: userMuted });
      call?.setMuted(userMuted);
    },
    toggleSpeechPause() {
      if (!inCall(get().view)) return;
      const speechPaused = !get().view.speechPaused;
      updateView({ speechPaused });
      if (call?.setSpeechPaused(speechPaused)) toast.erro(m.codex_voice_speech_lost());
    },
    setMode(mode) { if (inCall(get().view)) call?.setMode(mode); },
    loadSettings(server, force = false) {
      const captured = snapshot(server);
      if (settingsFor(captured).saving) return Promise.resolve();
      const pending = settingsRequests.get(server.id);
      if (pending && !force && sameServer(get().settingsByServer[server.id]?.server, captured)) return pending.promise;
      if (!force && settingsFor(captured).reply) return Promise.resolve();
      const sequence = ++settingsSequence;
      updateSettings(captured, view => ({ ...view, loading: true, error: null }));
      const promise = (async () => {
        try {
          const reply = await loadLiveVoiceSettings(captured);
          if (settingsRequests.get(captured.id)?.sequence !== sequence) return;
          updateSettings(captured, view => ({ ...view, reply, loading: false }));
          if (settingsOpen && sameServer(get().server, captured)) void loadModels(captured, reply.settings.codex_account);
        } catch (error) {
          if (settingsRequests.get(captured.id)?.sequence === sequence)
            updateSettings(captured, view => ({ ...view, loading: false, error: errorText(error) }));
        } finally {
          if (settingsRequests.get(captured.id)?.sequence === sequence) settingsRequests.delete(captured.id);
        }
      })();
      settingsRequests.set(captured.id, { sequence, promise });
      return promise;
    },
    async saveSettings(next) {
      const server = currentServer();
      if (!server || settingsFor(server).saving || settingsFor(server).loading) return;
      const previous = settingsFor(server).reply;
      if (!previous) return;
      const value = { ...next, organizer: { direct: { ...next.organizer.direct }, plan: { ...next.organizer.plan } } };
      modelsSequence++;
      updateSettings(server, view => ({ ...view, saving: true, saveError: null }));
      // Um GET anterior ao PUT não pode sobrescrever a escolha confirmada.
      settingsRequests.delete(server.id);
      try {
        const settings = await saveLiveVoiceSettings(server, value);
        updateSettings(server, view => ({ ...view, saving: false, saveError: null, reply: { ...previous, settings } }));
        if (settingsOpen && sameServer(get().server, server)) void loadModels(server, settings.codex_account);
      } catch (error) {
        updateSettings(server, view => ({ ...view, saving: false, saveError: errorText(error) }));
      }
    },
    openSettings() {
      const server = currentServer();
      if (!server) return;
      settingsOpen = true;
      void loadDetails(server);
      void get().loadSettings(server, true);
      void get().refreshMemory();
    },
    async refreshMemory() {
      const server = currentServer();
      if (!server) return;
      const sequence = ++memorySequence;
      updateView({ memory: { ...get().view.memory, loading: true, error: null } });
      try {
        const entries = await listVoiceMemory(server);
        if (sequence === memorySequence && sameServer(get().server, server))
          updateView({ memory: { ...get().view.memory, entries, loading: false, error: null } });
      } catch (error) {
        if (sequence === memorySequence && sameServer(get().server, server))
          updateView({ memory: { ...get().view.memory, loading: false, error: errorText(error) } });
      }
    },
    async forgetMemory(id) {
      const server = currentServer();
      if (!server) return;
      memorySequence++;
      updateView({ memory: { ...get().view.memory, loading: false, forgetError: null } });
      try {
        try { await forgetVoiceMemory(server, id); }
        catch (error) { if (errorCode(error) !== 'memory_unknown') throw error; }
        if (!sameServer(get().server, server)) return;
        memorySequence++;
        updateView({ memory: { ...get().view.memory, loading: false, entries: get().view.memory.entries.filter(entry => entry.id !== id) } });
      } catch (error) {
        if (sameServer(get().server, server)) updateView({ memory: { ...get().view.memory, forgetError: errorText(error) } });
      }
    },
    registerNavigation(navigate) {
      const registration = { navigate };
      navigator = registration;
      return () => { if (navigator === registration) { navigator = null; get().setRoute(null); } };
    },
    registerChat(context) {
      const registration = { ...context };
      chat = registration;
      set({ chatControls: context.controls ? { serverId: context.serverId, name: context.name } : null });
      return () => { if (chat === registration) { chat = null; set({ chatControls: null }); } };
    },
    setRoute(route) { set({ route }); publishScreen(); },
    setForeground(foreground) {
      set({ foreground });
      if (foreground && opening?.deadline !== undefined && get().view.phase === 'connecting' && Date.now() >= opening.deadline)
        void end({ code: 'opening_timeout' });
      publishScreen();
    },
    reconcileServers() {
      if (!get().server || currentServer()) return;
      if (inCall(get().view)) { void end({ code: 'server_removed', detail: m.chat_servidor_removido() }); return; }
      if (get().view.error) return;
      const saved = availableServer(get().server!.id);
      selectedServerId = saved?.id ?? null;
      if (saved) captureServer(saved);
      else set({ server: null, view: emptyView() });
    },
  };
});
