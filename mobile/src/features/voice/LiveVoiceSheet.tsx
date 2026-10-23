import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import { AccessibilityInfo, ActivityIndicator, Platform, Pressable, ScrollView, Text, View, useWindowDimensions } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import type { Credencial, LiveVoiceState, LiveVoiceSettings, LiveVoiceSettingsReply, ModelOption, Server, VoiceMemory } from '@hangar/core';
import { liveVoiceSpeaker, liveVoiceStatus, liveVoiceThoughtTail } from '@hangar/core';
import { Sheet } from '../../ui/Sheet';
import { Icon, type IconName } from '../../ui/Icon';
import { superficie } from '../../theme/superficie';
import { INK } from '../../theme/scale';
import { mkMarkdownStyle } from '../../chat/AssistantBubble';
import { Segmented } from '../config/Segmented';
import { QuietPill } from '../create/QuietPill';
import { VoiceBars, type VoiceBarsMode } from './VoiceBars';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import * as m from '../../paraglide/messages';

export interface LiveVoiceView {
  phase: 'idle' | 'connecting' | 'live' | 'closed';
  serverLabel: string;
  state: LiveVoiceState | null;
  error: { code: string; detail?: string } | null;
  muted: boolean;
  dictating?: boolean;
  levels: { input: number; output: number };
  liveSince: number | null;
  settings: {
    loading: boolean;
    reply: LiveVoiceSettingsReply | null;
    error: string | null;
    saving: boolean;
    saveError: string | null;
    accounts: Credencial[];
    models: ModelOption[];
    modelsError: string | null;
  };
  memory: { loading: boolean; entries: VoiceMemory[]; error: string | null; forgetError: string | null };
}

/** Níveis chegam a 10 Hz: só quem desenha as barras assina; a folha lê apenas quem está falando. */
export interface LiveVoiceLevels {
  get(): LiveVoiceView['levels'];
  subscribe(listener: () => void): () => void;
}

export interface LiveVoiceSheetProps {
  open: boolean;
  view: Omit<LiveVoiceView, 'levels'>;
  levels: LiveVoiceLevels;
  servers: Pick<Server, 'id' | 'label'>[];
  serverId: string | null;
  onSelectServer: (serverId: string) => void;
  onDismiss: () => void;
  onStart: () => void;
  onStop: () => void;
  onToggleMute: () => void;
  onMode: (mode: 'direto' | 'planejar') => void;
  onRetry: () => void;
  onSaveSettings: (next: LiveVoiceSettings) => void;
  onSettingsOpen: () => void;
  onForgetMemory: (id: string) => void;
}

type Mode = 'direct' | 'plan';
type Choice = { value: string | null; label: string };
const MODES: Mode[] = ['direct', 'plan'];
const ERRORS: Record<string, () => string> = {
  disabled: m.live_voice_error_disabled,
  codex_missing: m.live_voice_error_codex_missing,
  account_missing: m.live_voice_error_account_missing,
  unknown_account: m.live_voice_error_account_missing,
  connection_lost: m.live_voice_error_connection_lost,
  microphone: m.native_live_voice_error_microphone,
  playback: m.native_live_voice_error_playback,
  notifications_permission: m.native_live_voice_error_notifications_permission,
  audio_interrupted: m.native_live_voice_error_audio_interrupted,
  focus_lost: m.native_live_voice_error_focus_lost,
  foreground_required: m.native_live_voice_error_foreground_required,
  dictation_active: m.native_live_voice_error_dictation_active,
  audio_levels_unavailable: m.native_live_voice_error_audio_levels_unavailable,
  audio_restore: m.native_live_voice_error_audio_restore,
  opening_timeout: m.native_live_voice_error_opening_timeout,
  timeout: m.live_voice_error_timeout,
  taken: m.live_voice_taken,
};
const MEMORY_KINDS: Record<VoiceMemory['kind'], () => string> = {
  pessoa: m.live_voice_memory_kind_pessoa, preferencia: m.live_voice_memory_kind_preferencia,
  referencia: m.live_voice_memory_kind_referencia, projeto: m.live_voice_memory_kind_projeto,
};
const modeLabel = (mode: Mode) => mode === 'direct' ? m.live_voice_mode_direct() : m.live_voice_mode_plan();
const tierLabel = (tier: string) => tier === 'priority' ? m.live_voice_tier_fast() : tier === 'default' ? m.live_voice_tier_standard() : tier;

const STATUS: Record<ReturnType<typeof liveVoiceStatus>, () => string> = {
  connecting: m.live_voice_connecting, voice: m.live_voice_status_voice, muted: m.codex_voice_muted,
  you: m.live_voice_status_you, thinking: m.live_voice_status_thinking, searching: m.live_voice_status_searching,
  working: m.live_voice_status_working, listening: m.codex_voice_listening,
};
type Status = ReturnType<typeof liveVoiceStatus>;
// Mesma cor no equalizador e no rótulo: você no destaque, a voz em verde, agindo em aviso.
function statusColor(theme: ReturnType<typeof useUnistyles>['theme'], status: Status): string {
  switch (status) {
    case 'you': return theme.tokens.accent.base;
    case 'voice': return theme.tokens.status.success;
    case 'thinking': return theme.tokens.text.primary;
    case 'searching': case 'working': return theme.tokens.status.warning;
    default: return theme.tokens.text.muted;
  }
}
function barsMode(status: Status, levels: LiveVoiceView['levels']): VoiceBarsMode {
  switch (status) {
    case 'you': return { kind: 'level', value: levels.input };
    case 'voice': return { kind: 'level', value: levels.output };
    case 'thinking': case 'searching': case 'working': return { kind: 'wave', fast: true };
    case 'listening': return { kind: 'wave', fast: false };
    default: return { kind: 'rest' };
  }
}

// Único ponto que assina os níveis: quem fala vira cor, barras e rótulo aqui, sem redesenhar a folha.
function CallOrb({ levels, live, muted, activity }: {
  levels: LiveVoiceLevels; live: boolean; muted: boolean; activity: LiveVoiceState['activity'] | undefined;
}) {
  const { theme } = useUnistyles();
  const current = useSyncExternalStore(levels.subscribe, levels.get);
  const status = liveVoiceStatus(live, liveVoiceSpeaker(current.input, current.output, muted), muted, activity);
  const tone = statusColor(theme, status);
  return <>
    <View style={styles.orb}>
      <View style={[styles.orbRing, { backgroundColor: tone }]} />
      <View style={[styles.orbCore, { backgroundColor: tone }]} />
      <VoiceBars mode={barsMode(status, current)} color={tone} count={7} height={64} width={7} gap={theme.base.space[1.5]} />
    </View>
    <Text style={[styles.status, { color: tone }]} numberOfLines={1}>{STATUS[status]()}</Text>
  </>;
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return <View style={styles.section}><Text accessibilityRole="header" style={styles.label}>{title}</Text>{children}</View>;
}

function Action({ title, onPress, danger = false, selected = false, disabled = false }: {
  title: string; onPress: () => void; danger?: boolean; selected?: boolean; disabled?: boolean;
}) {
  return (
    <Pressable onPress={onPress} disabled={disabled} accessibilityRole="button" accessibilityLabel={title}
      accessibilityState={{ selected, disabled }} style={[styles.action, selected && styles.selected, disabled && styles.disabled]}>
      <Text style={[styles.actionText, danger && styles.danger]}>{title}</Text>
    </Pressable>
  );
}

function MenuField({ title, context, value, choices, disabled, onChange }: {
  title: string; context?: string; value: string | null; choices: Choice[]; disabled: boolean; onChange: (value: string | null) => void;
}) {
  // Um catálogo incompleto não pode esconder nem substituir a escolha guardada no servidor.
  const options = choices.some(c => c.value === value) ? choices
    : [{ value, label: value || m.live_voice_default() }, ...choices];
  return (
    <View style={styles.field}>
      <Text style={styles.detail}>{title}</Text>
      <QuietPill label={options.find(c => c.value === value)!.label} aria={context ? `${context}: ${title}` : title}
        menuTitle={context ? `${context}: ${title}` : title} disabled={disabled}
        actions={options.map((choice, i) => ({ id: String(i), title: choice.label, state: choice.value === value ? 'on' : 'off' }))}
        onAction={id => { const choice = options.find((_, i) => String(i) === id); if (!disabled && choice) onChange(choice.value); }} />
    </View>
  );
}

function RoundAction({ title, label, icon, onPress, danger = false, selected = false, disabled = false, expanded }: {
  title: string; label: string; icon: IconName; onPress: () => void; danger?: boolean; selected?: boolean; disabled?: boolean; expanded?: boolean;
}) {
  const { theme } = useUnistyles();
  return (
    <Pressable onPress={onPress} disabled={disabled} accessibilityRole="button" accessibilityLabel={label}
      accessibilityHint={title === label ? undefined : title}
      accessibilityState={{ selected, disabled, expanded }} style={[styles.round, disabled && styles.disabled]}>
      <View style={[styles.roundFace, selected && styles.roundOn, danger && styles.roundDanger]}>
        <Icon name={icon} size={26} color={danger ? INK.onAccent : selected ? theme.tokens.bg.base : theme.tokens.text.primary} />
      </View>
      <Text style={styles.roundLabel} numberOfLines={2}>{label}</Text>
    </Pressable>
  );
}

function Meter({ title, used }: { title: string; used: number }) {
  const { theme } = useUnistyles();
  const pct = Math.max(0, Math.min(100, Math.round(used)));
  const tone = pct >= 90 ? theme.tokens.status.error : pct >= 70 ? theme.tokens.status.warning : theme.tokens.accent.base;
  return (
    <View style={styles.meter} accessible accessibilityLabel={`${title}: ${pct}%`}>
      <View style={styles.meterRow}><Text style={styles.detail}>{title}</Text><Text style={styles.detail}>{pct}%</Text></View>
      <View style={styles.track}><View style={[styles.fill, { width: `${pct}%`, backgroundColor: tone }]} /></View>
    </View>
  );
}

export function LiveVoiceSheet({ open, view, levels, servers, serverId, onSelectServer, onDismiss, onStart, onStop, onToggleMute, onMode, onRetry,
  onSaveSettings, onSettingsOpen, onForgetMemory }: LiveVoiceSheetProps) {
  const { theme } = useUnistyles();
  const { height } = useWindowDimensions();
  // O `theme` é um Proxy novo a cada render; tokens e conversa só trocam quando o tema muda.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const markdownStyle = useMemo(() => mkMarkdownStyle(theme), [theme.tokens, theme.conversa]);
  // Tela de chamada e Detalhes se revezam na mesma folha; a escondida não fica montada.
  const [details, setDetails] = useState(false);
  const [serverPickerOpen, setServerPickerOpen] = useState(false);
  const scroll = useRef<ScrollView>(null);
  const [now, setNow] = useState(Date.now);
  const inCall = view.phase === 'connecting' || view.phase === 'live';
  const { settings, memory, state } = view;
  const reply = settings.reply;
  const disabled = settings.saving || settings.loading;
  const seconds = view.liveSince === null ? 0 : Math.max(0, Math.floor((now - view.liveSince) / 1000));
  const elapsed = `${String(Math.floor(seconds / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
  const thought = state?.activity && state.activity !== 'idle' ? liveVoiceThoughtTail(state.thought ?? '') : [];
  const muted = view.muted || !!view.dictating;
  const busy = state?.activity && state.activity !== 'idle';
  const doing = busy ? state.action?.text ?? state.progress ?? null : null;
  const effective = state?.effective;
  const windows = state ? ([[m.ctx_limite_5h(), state.limits.five_hour], [m.ctx_limite_7d(), state.limits.seven_day]] as const)
    .flatMap(([title, w]) => w ? [{ title, used: w.used_percent }] : []) : [];
  const muteTitle = view.dictating ? m.native_live_voice_dictation_hold_short() : muted ? m.live_voice_unmute() : m.live_voice_mute();

  useEffect(() => {
    if (!open || view.phase !== 'live') return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [open, view.phase, view.liveSince]);
  useEffect(() => { setDetails(false); }, [open, serverId]);
  useEffect(() => { setServerPickerOpen(false); }, [open, serverId, inCall, details]);
  useEffect(() => { if (serverPickerOpen) scroll.current?.scrollTo({ y: 0, animated: false }); }, [serverPickerOpen]);

  const save = (next: LiveVoiceSettings) => { if (!disabled) onSaveSettings(next); };
  const savePair = (mode: Mode, field: 'model' | 'effort' | 'tier', value: string | null) => {
    if (!reply || field === 'effort' && value === null) return;
    const current = reply.settings.organizer[mode];
    const model = field === 'model' ? settings.models.find(item => item.id === value) : undefined;
    const efforts = model?.efforts;
    const effort = efforts?.length && !efforts.includes(current.effort)
      ? model?.default_effort && efforts.includes(model.default_effort) ? model.default_effort : efforts[0] ?? current.effort
      : current.effort;
    save({ ...reply.settings, organizer: { ...reply.settings.organizer,
      [mode]: { ...current, effort, [field]: value } } });
  };
  const toggleDetails = () => {
    if (!details) onSettingsOpen();
    setDetails(!details);
  };

  const errors = <>
    {view.error ? (
      <View style={styles.notice}>
        <Text style={styles.danger} accessibilityRole="alert">{(ERRORS[view.error.code] ?? m.live_voice_error_failed)()}</Text>
        {view.error.detail ? <Text selectable style={styles.detail}>{view.error.detail}</Text> : null}
        <Action title={m.sync_retry()} onPress={onRetry} />
      </View>
    ) : null}
    {reply && settings.error ? (
      <View style={styles.notice}>
        <Text selectable style={styles.danger} accessibilityRole="alert">{settings.error}</Text>
        <Action title={m.sync_retry()} onPress={onRetry} />
      </View>
    ) : null}
    {state?.error ? <Text selectable style={styles.danger} accessibilityRole="alert">{state.error.text}</Text> : null}
  </>;

  // Antes de ligar, o centro diz se dá para ligar; a falha de carga ocupa o lugar do estado.
  const idleStatus = settings.loading && !reply ? m.comum_carregando()
    : !reply ? null : !reply.codex ? m.live_voice_error_codex_missing() : !reply.enabled ? m.live_voice_error_no_account() : m.native_live_voice_ready();
  const canStart = !inCall && !!reply && reply.codex && reply.enabled;
  // Anuncia o que muda de fato: nem o cronômetro (todo segundo) nem quem fala (a cada sílaba).
  const announcement = [inCall ? STATUS[liveVoiceStatus(view.phase === 'live', 'idle', muted, state?.activity)]() : idleStatus ?? m.live_voice_error_failed(),
    doing ?? '', busy && state.progress !== doing ? state.progress ?? '' : ''].filter(Boolean).join(', ');
  const announced = useRef(announcement);
  useEffect(() => {
    // Região viva só existe no Android; no iOS o VoiceOver precisa do anúncio explícito.
    if (announced.current === announcement) return;
    announced.current = announcement;
    if (Platform.OS === 'ios' && open && inCall) AccessibilityInfo.announceForAccessibility(announcement);
  }, [announcement, open, inCall]);

  const callView = <>
    <ScrollView ref={scroll} style={styles.scroll} contentContainerStyle={styles.stageScroll} keyboardShouldPersistTaps="handled">
      {serverPickerOpen ? <View accessibilityRole="radiogroup" accessibilityLabel={m.live_voice_server()}>
        {servers.map(server => <Pressable key={server.id} disabled={inCall}
          onPress={() => { if (!inCall) { setServerPickerOpen(false); onSelectServer(server.id); } }}
          accessibilityRole="radio" accessibilityLabel={server.label}
          accessibilityState={{ selected: server.id === serverId, disabled: inCall }} style={styles.serverItem}>
          <Text style={[styles.serverName, server.id === serverId && styles.serverChosen]} numberOfLines={1}>{server.label}</Text>
          {server.id === serverId ? <Icon name="Check" size={16} color={theme.tokens.accent.base} /> : null}
        </Pressable>)}
      </View> : null}
      {errors}
      <View style={styles.stage}>
        <View style={styles.stage} accessible accessibilityLiveRegion="polite" accessibilityLabel={announcement}>
          {inCall ? <CallOrb levels={levels} live={view.phase === 'live'} muted={muted} activity={state?.activity} /> : <View style={styles.orb}>
            <View style={[styles.orbRing, { backgroundColor: theme.tokens.text.muted }]} />
            <View style={[styles.orbCore, { backgroundColor: theme.tokens.text.muted }]} />
            <VoiceBars mode={{ kind: 'rest' }} color={theme.tokens.text.muted} count={7} height={64} width={7} gap={theme.base.space[1.5]} />
          </View>}
          {inCall ? null
            : idleStatus ? <Text style={[styles.status, { color: reply && (!reply.codex || !reply.enabled) ? theme.tokens.status.error : theme.tokens.text.secondary }]}>{idleStatus}</Text>
              : <Text style={[styles.status, { color: theme.tokens.status.error }]} accessibilityRole="alert">{m.live_voice_error_failed()}</Text>}
          {view.phase === 'live' ? <Text style={styles.time}>{elapsed}</Text>
            : view.phase === 'connecting' || settings.loading && !reply ? <ActivityIndicator size="small" color={theme.tokens.accent.base} /> : null}
          {doing ? <Text selectable style={styles.doing} numberOfLines={2}>{doing}</Text> : null}
        </View>
        {state?.action && state.progress && busy ? <Text selectable style={styles.doing}>{state.progress}</Text> : null}
        {thought.map((line, i) => <Text key={i} selectable style={styles.thought} numberOfLines={1}>{line}</Text>)}
        {inCall && view.dictating ? <Text style={styles.doing}>{m.native_live_voice_dictation_hold()}</Text> : null}
        {!inCall && !reply && settings.error ? <Text selectable style={styles.detail}>{settings.error}</Text> : null}
        {!inCall && !reply && !settings.loading && !view.error ? <Action title={m.sync_retry()} onPress={onRetry} /> : null}
      </View>
      {state?.plan ? <View style={styles.card}>
        <Text accessibilityRole="header" style={styles.label}>{m.live_voice_plan()}</Text>
        <EnrichedMarkdownText markdown={state.plan.markdown} markdownStyle={markdownStyle} flavor="github" />
      </View> : null}
    </ScrollView>
    {state?.draft ? <View style={styles.draft}>
      <Text style={styles.draftLabel}>{m.live_voice_draft()}</Text>
      <Text selectable style={styles.text}>{state.draft}</Text>
    </View> : null}
    {inCall ? (
      <View style={styles.dock}>
        {state && state.mode !== null ? <View style={styles.modes}><Segmented label={m.native_live_voice_mode()} value={state.mode}
          options={MODES.map(mode => ({ v: mode, label: modeLabel(mode) }))}
          onChange={mode => onMode(mode === 'direct' ? 'direto' : 'planejar')} /></View> : null}
        <View style={styles.controls}>
          <RoundAction title={muteTitle} label={muteTitle} icon={muted ? 'MicOff' : 'Mic'} onPress={onToggleMute} selected={muted} disabled={!!view.dictating} />
          <RoundAction title={m.native_live_voice_settings_memory()} label={m.native_live_voice_details()} icon="SlidersHorizontal"
            onPress={toggleDetails} expanded={false} />
          <RoundAction title={m.live_voice_stop()} label={m.live_voice_stop()} icon="PhoneOff" onPress={onStop} danger />
        </View>
      </View>
    ) : (
      <View style={styles.dock}>
        {canStart ? <Pressable onPress={onStart} accessibilityRole="button" accessibilityLabel={m.live_voice_start()} style={styles.start}>
          <Icon name="AudioLines" size={22} color={INK.onAccent} /><Text style={styles.startText}>{m.live_voice_start()}</Text>
        </Pressable> : null}
        <Pressable onPress={toggleDetails} accessibilityRole="button" accessibilityLabel={m.native_live_voice_settings_memory()}
          accessibilityState={{ expanded: false }} style={styles.link}>
          <Text style={styles.linkText}>{m.native_live_voice_settings_memory()}</Text>
        </Pressable>
      </View>
    )}
  </>;

  const detailsView = (
    <ScrollView ref={scroll} style={styles.scroll} contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
      {errors}
      {effective && (effective.model || effective.effort || effective.tier) ? <Section title={m.native_live_voice_effective()}>
        <View style={styles.group}>
          {effective.model ? <Row title={m.live_voice_model()} value={effective.model} /> : null}
          {effective.effort ? <Row title={m.live_voice_effort()} value={effective.effort} /> : null}
          {effective.tier ? <Row title={m.live_voice_tier()} value={tierLabel(effective.tier)} /> : null}
        </View>
      </Section> : null}
      {windows.length ? <Section title={m.native_live_voice_limits()}>
        <View style={[styles.group, styles.meters]}>{windows.map(w => <Meter key={w.title} title={w.title} used={w.used} />)}</View>
      </Section> : null}
      {state?.backstage.length ? <Section title={m.live_voice_backstage()}>
        <View style={[styles.group, styles.lines]}>
          {state.backstage.slice(-6).map((line, i) => <Text key={i} selectable style={line.kind === 'heard' ? styles.text : styles.detail}>{line.text}</Text>)}
        </View>
      </Section> : null}
      {state?.followed.length ? <Section title={m.live_voice_followed()}>
        <View style={[styles.group, styles.lines]}>
          {state.followed.map(item => <Text key={`${item.server}::${item.name}`} selectable style={styles.detail}>
            {item.server ? `${item.server}::${item.name}` : item.name}
          </Text>)}
        </View>
      </Section> : null}
      {settings.loading ? <Text style={styles.detail}>{m.comum_carregando()}</Text> : null}
      {settings.error && !reply ? <Text selectable style={styles.danger} accessibilityRole="alert">{settings.error}</Text> : null}
      {reply ? (
        <Section title={m.live_voice_settings()}>
          <Text style={styles.detail}>{m.native_live_voice_next_call()}</Text>
          <View style={styles.group}>
            <MenuField title={m.live_voice_account()} value={reply.settings.codex_account} disabled={disabled}
              choices={settings.accounts.filter(c => c.tipo === 'codex').map(c => ({ value: c.codex_account ?? c.id, label: c.apelido || c.nome_natural || c.nome }))}
              onChange={value => { if (value !== null) save({ ...reply.settings, codex_account: value }); }} />
            <MenuField title={m.live_voice_voice()} value={reply.settings.voice} disabled={disabled}
              choices={[{ value: null, label: m.live_voice_default() }, ...reply.voices.map(value => ({ value, label: value }))]}
              onChange={voice => save({ ...reply.settings, voice })} />
            {reply.settings.style ? <MenuField title={m.live_voice_style()} value={reply.settings.style} disabled={disabled}
              choices={[{ value: 'conversational', label: m.live_voice_style_conversational() }, { value: 'direct', label: m.live_voice_style_direct() }]}
              onChange={style => save({ ...reply.settings, style: style === 'direct' ? 'direct' : 'conversational' })} /> : null}
          </View>
          {reply.settings.style ? <Text style={styles.detail}>{reply.settings.style === 'direct' ? m.live_voice_style_direct_hint()
            : m.live_voice_style_conversational_hint()} {m.live_voice_style_next_call()}</Text> : null}
          {!settings.loading && !settings.accounts.some(c => c.tipo === 'codex') ? <Text style={styles.detail}>{m.native_live_voice_accounts_empty()}</Text> : null}
          {settings.modelsError ? <>
            <Text style={styles.danger} accessibilityRole="alert">{m.comum_falha_carregar_modelos()}</Text>
            <Text selectable style={styles.detail}>{settings.modelsError}</Text>
          </> : !settings.loading && settings.models.length === 0 ? <Text style={styles.detail}>{m.native_live_voice_models_empty()}</Text> : null}
        </Section>
      ) : null}
      {reply ? MODES.map(mode => {
        const pair = reply.settings.organizer[mode];
        const model = settings.models.find(item => item.id === pair.model);
        const efforts = model ? model.efforts ?? [] : settings.models.reduce<string[] | null>((all, item) =>
          all === null ? [...item.efforts ?? []] : all.filter(effort => item.efforts?.includes(effort)), null) ?? [];
        return (
          <Section key={mode} title={modeLabel(mode)}><View style={styles.group}>
            <MenuField title={m.live_voice_model()} context={modeLabel(mode)} value={pair.model} disabled={disabled}
              choices={[{ value: null, label: m.live_voice_default() }, ...settings.models.map(item => ({ value: item.id, label: item.name ?? item.id }))]}
              onChange={value => savePair(mode, 'model', value)} />
            <MenuField title={m.live_voice_effort()} context={modeLabel(mode)} value={pair.effort} disabled={disabled}
              choices={efforts.map(value => ({ value, label: value }))} onChange={value => savePair(mode, 'effort', value)} />
            <MenuField title={m.live_voice_tier()} context={modeLabel(mode)} value={pair.tier} disabled={disabled}
              choices={[{ value: null, label: m.live_voice_default() }, { value: 'default', label: m.live_voice_tier_standard() },
                { value: 'priority', label: m.live_voice_tier_fast() }]} onChange={value => savePair(mode, 'tier', value)} />
          </View></Section>
        );
      }) : null}
      {settings.saving ? <Text style={styles.detail}>{m.native_live_voice_saving()}</Text> : null}
      {settings.saveError ? <Text selectable style={styles.danger} accessibilityRole="alert">{settings.saveError}</Text> : null}
      <Section title={m.live_voice_memory_title()}>
        {memory.loading ? <Text style={styles.detail}>{m.comum_carregando()}</Text> : null}
        {memory.error ? <>
          <Text style={styles.danger} accessibilityRole="alert">{m.live_voice_memory_error()}</Text>
          <Text selectable style={styles.detail}>{memory.error}</Text>
        </> : null}
        {memory.forgetError ? <>
          <Text style={styles.danger} accessibilityRole="alert">{m.live_voice_memory_forget_failed()}</Text>
          <Text selectable style={styles.detail}>{memory.forgetError}</Text>
        </> : null}
        {!memory.loading && !memory.error && memory.entries.length === 0 ? <Text style={styles.detail}>{m.live_voice_memory_empty()}</Text> : null}
        {memory.entries.length ? <View style={styles.group}>
          {memory.entries.map(entry => (
            <View key={entry.id} style={styles.memory}>
              <View style={styles.heading}>
                <Text style={styles.label}>{(MEMORY_KINDS[entry.kind] ?? (() => entry.kind))()}</Text>
                <Text selectable style={styles.text}>{entry.text}</Text>
                {entry.kind === 'projeto' ? <Text selectable style={styles.detail}>{entry.confirmed}</Text> : null}
              </View>
              <Pressable onPress={() => onForgetMemory(entry.id)} accessibilityRole="button"
                accessibilityLabel={m.live_voice_memory_forget({ text: entry.text })} style={styles.iconButton}>
                <Icon name="Trash2" size={20} />
              </Pressable>
            </View>
          ))}
        </View> : null}
      </Section>
      <Action title={m.native_live_voice_reload_settings_memory()} onPress={onSettingsOpen} />
    </ScrollView>
  );

  return (
    <Sheet open={open} sizes={['large']} onDismiss={onDismiss} scrollable>
      {/* A folha fechada continua montada: as barras param com ela. */}
      <WorkingMarkPaused.Provider value={!open}>
      <View style={[styles.wrap, { height: height * 0.84 }]}>
        <View style={styles.header}>
          {details ? <Pressable onPress={toggleDetails} accessibilityRole="button" accessibilityLabel={m.native_live_voice_settings_memory()}
            accessibilityHint={m.comum_voltar()} accessibilityState={{ expanded: true }} style={styles.iconButton}>
            <Icon name="ChevronLeft" size={24} />
          </Pressable> : null}
          <View style={styles.heading}>
            <Text style={details ? styles.title : styles.kicker} accessibilityRole="header">{details ? m.native_live_voice_details() : m.live_voice_open()}</Text>
            {details ? null : servers.length > 0 ? <View style={styles.server}>
              <QuietPill label={view.serverLabel} aria={m.live_voice_server()} disabled={inCall}
                onPress={() => { if (!inCall) setServerPickerOpen(!serverPickerOpen); }} />
            </View> : <Text style={styles.serverName} selectable>{m.live_voice_server()}: {view.serverLabel}</Text>}
          </View>
          <Pressable onPress={onDismiss} accessibilityRole="button" accessibilityLabel={m.native_live_voice_close()}
            style={styles.iconButton}><Icon name="ChevronDown" size={24} /></Pressable>
        </View>
        {details ? detailsView : callView}
      </View>
      </WorkingMarkPaused.Provider>
    </Sheet>
  );
}

function Row({ title, value }: { title: string; value: string }) {
  return <View style={styles.field}><Text style={styles.detail}>{title}</Text><Text selectable style={styles.text}>{value}</Text></View>;
}

const styles = StyleSheet.create((theme) => ({
  wrap: { paddingHorizontal: theme.base.space[4], paddingTop: theme.base.space[2], gap: theme.base.space[3] },
  header: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  heading: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  kicker: { ...theme.type.meta, color: theme.tokens.text.muted },
  title: { ...theme.type.heading, color: theme.tokens.text.primary },
  label: { ...theme.type.label, color: theme.tokens.text.secondary },
  text: { ...theme.type.label, fontWeight: theme.base.weight.normal, color: theme.tokens.text.primary },
  detail: { ...theme.type.meta, color: theme.tokens.text.muted },
  server: { flexDirection: 'row' },
  serverItem: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], minHeight: 48,
    paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.md },
  serverName: { flex: 1, ...theme.type.body, fontWeight: theme.base.weight.medium, color: theme.tokens.text.primary },
  serverChosen: { fontWeight: theme.base.weight.bold },
  scroll: { flex: 1 },
  stageScroll: { flexGrow: 1, justifyContent: 'center', gap: theme.base.space[4], paddingVertical: theme.base.space[4] },
  stage: { alignItems: 'center', gap: theme.base.space[2] },
  orb: { width: 176, height: 176, alignItems: 'center', justifyContent: 'center', marginBottom: theme.base.space[2] },
  orbRing: { position: 'absolute', width: 176, height: 176, borderRadius: theme.base.radius.full, opacity: 0.1 },
  orbCore: { position: 'absolute', width: 128, height: 128, borderRadius: theme.base.radius.full, opacity: 0.14 },
  status: { ...theme.type.title, textAlign: 'center' },
  time: { ...theme.type.label, color: theme.tokens.text.secondary, fontVariant: ['tabular-nums'] },
  doing: { ...theme.type.meta, color: theme.tokens.text.secondary, textAlign: 'center' },
  thought: { ...theme.type.caption, color: theme.tokens.text.muted, textAlign: 'center' },
  notice: { gap: theme.base.space[2], padding: theme.base.space[3], borderRadius: theme.base.radius.md, backgroundColor: superficie(theme) },
  card: { gap: theme.base.space[2], padding: theme.base.space[3], borderRadius: theme.base.radius.md, backgroundColor: superficie(theme) },
  draft: { gap: theme.base.space[1], padding: theme.base.space[3], borderRadius: theme.base.radius.md,
    borderWidth: 1, borderColor: theme.tokens.status.warning, backgroundColor: superficie(theme) },
  draftLabel: { ...theme.type.label, color: theme.tokens.status.warning },
  dock: { gap: theme.base.space[4], paddingBottom: theme.base.space[6] },
  modes: { alignSelf: 'center', minWidth: 220 },
  controls: { flexDirection: 'row', justifyContent: 'space-around' },
  round: { width: 96, alignItems: 'center', gap: theme.base.space[1.5] },
  roundFace: { width: 68, height: 68, borderRadius: theme.base.radius.full, alignItems: 'center', justifyContent: 'center',
    backgroundColor: superficie(theme, 0.8) },
  roundOn: { backgroundColor: theme.tokens.text.primary },
  roundDanger: { backgroundColor: theme.tokens.status.error },
  roundLabel: { ...theme.type.caption, color: theme.tokens.text.secondary, textAlign: 'center' },
  start: { minHeight: 56, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: theme.base.space[2],
    borderRadius: theme.base.radius.full, backgroundColor: theme.tokens.accent.base },
  startText: { ...theme.type.control, fontSize: theme.base.text.base, color: INK.onAccent },
  link: { minHeight: 44, alignItems: 'center', justifyContent: 'center' },
  linkText: { ...theme.type.label, color: theme.tokens.accent.base },
  content: { gap: theme.base.space[5], paddingBottom: theme.base.space[6] },
  section: { gap: theme.base.space[2] },
  group: { paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md, backgroundColor: superficie(theme) },
  meters: { flexDirection: 'row', gap: theme.base.space[4], paddingVertical: theme.base.space[3] },
  lines: { gap: theme.base.space[1.5], paddingVertical: theme.base.space[3] },
  memory: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingVertical: theme.base.space[2] },
  iconButton: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center' },
  action: { flexGrow: 1, minHeight: 44, paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2],
    borderWidth: 1, borderColor: theme.tokens.border.default, borderRadius: theme.base.radius.md, alignItems: 'center', justifyContent: 'center' },
  actionText: { ...theme.type.label, color: theme.tokens.text.primary },
  selected: { borderColor: theme.tokens.accent.base, backgroundColor: theme.tokens.accent.dim },
  disabled: { opacity: 0.5 },
  danger: { ...theme.type.label, color: theme.tokens.status.error },
  field: { minHeight: 44, flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', justifyContent: 'space-between',
    columnGap: theme.base.space[3], rowGap: theme.base.space[1] },
  meter: { flex: 1, gap: theme.base.space[1] },
  meterRow: { flexDirection: 'row', justifyContent: 'space-between' },
  track: { height: 4, borderRadius: theme.base.radius.full, backgroundColor: theme.tokens.border.subtle, overflow: 'hidden' },
  fill: { height: 4 },
}));
