import { probeServerResponse } from './api';
import type { Server } from './servers';

/** IDs dos peers da máquina da chamada, não os IDs locais do aparelho. */
export type LiveVoiceScreen = { server: string; name: string } | null;
export type LiveVoiceMode = 'direto' | 'planejar';
export type LiveVoicePhase = 'connecting' | 'live' | 'closed';
export type LiveVoiceActivity = 'idle' | 'thinking' | 'searching' | 'working';
export type LiveVoiceRateWindow = { used_percent: number; resets_at: number | null } | null;
export interface LiveVoiceModel { model: string | null; effort: string; tier: string | null }
export type LiveVoiceStyle = 'conversational' | 'direct';
export const LIVE_VOICE_STYLES: readonly LiveVoiceStyle[] = ['conversational', 'direct'];
export interface LiveVoiceSettings {
  voice: string | null;
  codex_account: string;
  organizer: { direct: LiveVoiceModel; plan: LiveVoiceModel };
  /** Ausente: servidor anterior ao estilo, que o ignoraria; a tela esconde a escolha. */
  style?: LiveVoiceStyle;
}
export interface LiveVoiceSettingsReply {
  enabled: boolean;
  codex: boolean;
  jev: boolean;
  voices: string[];
  settings: LiveVoiceSettings;
  call: { active: boolean; client: string | null };
}
export interface VoiceMemory {
  id: string;
  kind: 'pessoa' | 'preferencia' | 'referencia' | 'projeto';
  text: string;
  learned: string;
  confirmed: string;
}
export interface LiveVoiceState {
  phase: string;
  mode: 'direct' | 'plan' | null;
  activity: LiveVoiceActivity | null;
  draft: string | null;
  error: { code: string; text: string } | null;
  plan: { path: string; markdown: string } | null;
  effective: { model: string | null; effort: string | null; tier: string | null } | null;
  context: { used: number; 'window': number | null } | null;
  limits: { five_hour: LiveVoiceRateWindow; seven_day: LiveVoiceRateWindow };
  backstage: { kind: 'heard' | 'result' | 'answer' | null; text: string }[];
  thought: string;
  action: { kind: 'tool' | 'search' | 'command' | null; text: string } | null;
  progress?: string | null;
  followed: { server: string; name: string }[];
  server: string;
  client: string;
  memory?: number;
}
export interface LiveVoiceAction { id: string; label: string; description: string }
export interface LiveVoiceHandlers {
  phase(phase: LiveVoicePhase): void;
  state(state: LiveVoiceState): void;
  failed(code: string, detail?: string): void;
  taken(): void;
  switchTo(target: { server: string; baseUrl: string | null; name: string }): boolean;
  /** Só a exibição recebe ganho; o protocolo leva RMS cru. */
  levels(levels: { input: number; output: number }): void;
  actions(): LiveVoiceAction[];
  runAction(id: string, arg: string | null): true | string;
}
export const LIVE_VOICE_CAPS = ['switch_session', 'hangar_actions', 'hangar_action'] as const;
export type LiveVoiceClientMessage =
  | { type: 'hello'; client: 'mobile' | 'pwa'; screen: LiveVoiceScreen; caps: readonly string[]; actions: LiveVoiceAction[] }
  | { type: 'offer'; sdp: string }
  | { type: 'live' | 'stop' | 'ping' }
  | { type: 'level'; input: number }
  | { type: 'screen'; screen: LiveVoiceScreen }
  | { type: 'tool_result'; call: number; ok: boolean; text: string }
  | { type: 'mode'; mode: LiveVoiceMode };
export type LiveVoiceServerMessage =
  | { type: 'answer'; sdp: string }
  | { type: 'state'; state: LiveVoiceState }
  | { type: 'tool'; call: number; name: string; args: unknown }
  | { type: 'pong' | 'taken' | 'closed' }
  | { type: 'error'; code: string; detail?: string };

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function finite(value: unknown): value is number { return typeof value === 'number' && Number.isFinite(value); }
function nullableString(value: unknown): value is string | null { return value === null || typeof value === 'string'; }
function nullableNumber(value: unknown): value is number | null { return value === null || finite(value); }
function model(value: unknown): value is LiveVoiceModel {
  return object(value) && nullableString(value.model) && typeof value.effort === 'string' && nullableString(value.tier);
}
function settings(value: unknown): value is LiveVoiceSettings {
  return object(value) && nullableString(value.voice) && typeof value.codex_account === 'string' && !!value.codex_account
    && object(value.organizer) && model(value.organizer.direct) && model(value.organizer.plan)
    && (value.style === undefined || LIVE_VOICE_STYLES.includes(value.style as LiveVoiceStyle));
}
function settingsReply(value: unknown): value is LiveVoiceSettingsReply {
  return object(value) && typeof value.enabled === 'boolean' && typeof value.codex === 'boolean' && typeof value.jev === 'boolean'
    && Array.isArray(value.voices) && value.voices.every(v => typeof v === 'string') && settings(value.settings)
    && object(value.call) && typeof value.call.active === 'boolean' && nullableString(value.call.client);
}
function memory(value: unknown): value is VoiceMemory {
  return object(value) && typeof value.id === 'string' && typeof value.text === 'string'
    && typeof value.learned === 'string' && typeof value.confirmed === 'string'
    && (value.kind === 'pessoa' || value.kind === 'preferencia' || value.kind === 'referencia' || value.kind === 'projeto');
}
function rateWindow(value: unknown): value is LiveVoiceRateWindow {
  return value === null || object(value) && finite(value.used_percent) && nullableNumber(value.resets_at);
}
function screen(value: unknown): boolean {
  return object(value) && typeof value.server === 'string' && typeof value.name === 'string';
}
export function isLiveVoiceState(value: unknown): value is LiveVoiceState {
  if (!object(value)) return false;
  return typeof value.phase === 'string' && (value.mode === null || value.mode === 'direct' || value.mode === 'plan')
    && (value.activity === null || value.activity === 'idle' || value.activity === 'thinking' || value.activity === 'searching' || value.activity === 'working')
    && nullableString(value.draft)
    && (value.error === null || object(value.error) && typeof value.error.code === 'string' && typeof value.error.text === 'string')
    && (value.plan === null || object(value.plan) && typeof value.plan.path === 'string' && typeof value.plan.markdown === 'string')
    && (value.effective === null || object(value.effective) && nullableString(value.effective.model) && nullableString(value.effective.effort) && nullableString(value.effective.tier))
    && (value.context === null || object(value.context) && finite(value.context.used) && nullableNumber(value.context['window']))
    && object(value.limits) && rateWindow(value.limits.five_hour) && rateWindow(value.limits.seven_day)
    && Array.isArray(value.backstage) && value.backstage.every(v => object(v) && typeof v.text === 'string' && (v.kind === null || v.kind === 'heard' || v.kind === 'result' || v.kind === 'answer'))
    && typeof value.thought === 'string'
    && (value.action === null || object(value.action) && typeof value.action.text === 'string' && (value.action.kind === null || value.action.kind === 'tool' || value.action.kind === 'search' || value.action.kind === 'command'))
    && (value.progress === undefined || nullableString(value.progress))
    && Array.isArray(value.followed) && value.followed.every(screen)
    && typeof value.server === 'string' && typeof value.client === 'string'
    && (value.memory === undefined || finite(value.memory));
}

function neutralEnum(value: unknown, known: readonly string[]): unknown {
  return typeof value === 'string' && !known.includes(value) ? null : value;
}
function parseLiveVoiceState(value: unknown): LiveVoiceState | null {
  if (!object(value)) return null;
  // Enum futuro perde só a categoria, não os dados úteis da chamada.
  const state = {
    ...value,
    mode: neutralEnum(value.mode, ['direct', 'plan']),
    activity: neutralEnum(value.activity, ['idle', 'thinking', 'searching', 'working']),
    backstage: Array.isArray(value.backstage) ? value.backstage.map(item => object(item)
      ? { ...item, kind: neutralEnum(item.kind, ['heard', 'result', 'answer']) } : item) : value.backstage,
    action: object(value.action) ? { ...value.action, kind: neutralEnum(value.action.kind, ['tool', 'search', 'command']) } : value.action,
  };
  return isLiveVoiceState(state) ? state : null;
}

/** Corpo malformado nunca vira um retrato parcial aceito pelo cliente. */
export function parseLiveVoiceMessage(raw: string): LiveVoiceServerMessage | null {
  let value: unknown;
  try { value = JSON.parse(raw); } catch { return null; }
  if (!object(value)) return null;
  switch (value.type) {
    case 'answer': return typeof value.sdp === 'string' && value.sdp.trim() ? { type: 'answer', sdp: value.sdp } : null;
    case 'state': {
      const state = parseLiveVoiceState(value.state);
      return state ? { type: 'state', state } : null;
    }
    case 'tool': return Number.isSafeInteger(value.call) && finite(value.call) && value.call >= 0 && typeof value.name === 'string'
      ? { type: 'tool', call: value.call, name: value.name, args: value.args } : null;
    case 'error': return typeof value.code === 'string' && value.code && (value.detail == null || typeof value.detail === 'string')
      ? { type: 'error', code: value.code, detail: value.detail ?? undefined } : null;
    case 'pong': case 'taken': case 'closed': return { type: value.type };
    default: return null;
  }
}

export class LiveVoiceRequestError extends Error {
  constructor(readonly code: string, readonly detail?: string, readonly status?: number) {
    super(detail ? `${code}: ${detail}` : code);
    this.name = 'LiveVoiceRequestError';
  }
}
async function request(server: Server, path: string, init?: RequestInit): Promise<unknown> {
  const response = await probeServerResponse(server, path, init);
  let body: unknown;
  try { body = await response.json(); }
  catch { throw new LiveVoiceRequestError(response.ok ? 'invalid_response' : `http_${response.status}`, undefined, response.status); }
  if (!response.ok) {
    const root = object(body) ? body : {};
    const envelope = object(root.detail) ? root.detail : root;
    const code = typeof envelope.error_code === 'string' ? envelope.error_code
      : typeof envelope.code === 'string' ? envelope.code : `http_${response.status}`;
    const detail = typeof envelope.detail === 'string' ? envelope.detail
      : typeof envelope.message === 'string' ? envelope.message : typeof root.detail === 'string' ? root.detail : undefined;
    throw new LiveVoiceRequestError(code, detail, response.status);
  }
  return body;
}
export async function loadLiveVoiceSettings(server: Server, signal?: AbortSignal): Promise<LiveVoiceSettingsReply> {
  const value = await request(server, '/api/voice/settings', signal ? { signal } : undefined);
  if (!settingsReply(value)) throw new LiveVoiceRequestError('invalid_response');
  return value;
}
/** PUT inteiro: `null` é uma escolha, não um campo para omitir. */
export async function saveLiveVoiceSettings(server: Server, value: LiveVoiceSettings, signal?: AbortSignal): Promise<LiveVoiceSettings> {
  if (!settings(value)) throw new LiveVoiceRequestError('invalid_body');
  const reply = await request(server, '/api/voice/settings', { method: 'PUT', body: JSON.stringify(value), ...(signal ? { signal } : {}) });
  if (!object(reply) || !settings(reply.settings)) throw new LiveVoiceRequestError('invalid_response');
  return reply.settings;
}
export async function listVoiceMemory(server: Server, signal?: AbortSignal): Promise<VoiceMemory[]> {
  const value = await request(server, '/api/voice/memory', signal ? { signal } : undefined);
  if (!object(value) || !Array.isArray(value.entries) || !value.entries.every(memory)) throw new LiveVoiceRequestError('invalid_response');
  return value.entries;
}
export async function forgetVoiceMemory(server: Server, id: string, signal?: AbortSignal): Promise<void> {
  if (!id) throw new LiveVoiceRequestError('invalid_body');
  const value = await request(server, `/api/voice/memory/${encodeURIComponent(id)}`, { method: 'DELETE', ...(signal ? { signal } : {}) });
  if (!object(value) || value.ok !== true) throw new LiveVoiceRequestError('invalid_response');
}

export interface LiveVoiceAudioSample { audioLevel?: number; totalAudioEnergy?: number; totalSamplesDuration?: number }
/** `null` não é silêncio: faltou medição ou ainda não há duas amostras para o delta. */
export function liveVoiceRms(sample: LiveVoiceAudioSample, previous?: LiveVoiceAudioSample): number | null {
  if (finite(sample.audioLevel) && sample.audioLevel >= 0 && sample.audioLevel <= 1) return sample.audioLevel;
  if (!previous || !finite(sample.totalAudioEnergy) || !finite(sample.totalSamplesDuration)
    || !finite(previous.totalAudioEnergy) || !finite(previous.totalSamplesDuration)) return null;
  const duration = sample.totalSamplesDuration - previous.totalSamplesDuration;
  const energy = sample.totalAudioEnergy - previous.totalAudioEnergy;
  if (duration <= 0 || energy < 0 || sample.totalAudioEnergy < 0 || sample.totalSamplesDuration < 0) return null;
  const rms = Math.sqrt(energy / duration);
  return Number.isFinite(rms) ? Math.min(1, rms) : null;
}
export function liveVoiceBarLevel(raw: number): number { return Math.round(Math.min(1, Math.max(0, raw) * 5) * 50) / 50; }

export type LiveVoiceSpeaker = 'you' | 'voice' | 'idle';
export type LiveVoiceStatus = 'connecting' | 'voice' | 'muted' | 'you' | 'thinking' | 'searching' | 'working' | 'listening';
export const LIVE_VOICE_SPEAKER_HOLD_MS = 300;
export function liveVoiceSpeaker(input: number, output: number, muted: boolean): LiveVoiceSpeaker {
  const heard = muted ? 0 : input;
  if (heard > 0.08 && heard >= output) return 'you';
  if (output > 0.08 && output > heard) return 'voice';
  return 'idle';
}
export function settledLiveVoiceSpeaker(shown: LiveVoiceSpeaker, since: number, raw: LiveVoiceSpeaker, now: number): [LiveVoiceSpeaker, number] {
  if (raw === shown) return [shown, now];
  return now - since >= LIVE_VOICE_SPEAKER_HOLD_MS ? [raw, now] : [shown, since];
}
export function liveVoiceStatus(live: boolean, who: LiveVoiceSpeaker, muted: boolean, activity: LiveVoiceActivity | null | undefined): LiveVoiceStatus {
  if (!live) return 'connecting';
  if (who === 'voice') return 'voice';
  if (muted) return 'muted';
  if (who === 'you') return 'you';
  return activity && activity !== 'idle' ? activity : 'listening';
}
export function liveVoiceThoughtTail(thought: string, lines = 3): string[] {
  return thought.split('\n').map(line => line.trim().replace(/^\*+|\*+$/g, '').trim()).filter(Boolean).slice(-lines);
}
