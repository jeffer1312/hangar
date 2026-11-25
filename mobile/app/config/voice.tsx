import { useEffect, useRef, useState } from 'react';
import { Linking, Switch, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { MenuView } from '@react-native-menu/menu';
import { ehEstilo, listarVozesTts, parseTranscriptionProviders, saldoTts, type TtsVoz } from '@hangar/core';
import { BlockHead, Box, Chip, ConfigRow, Disclosure, Muted, RowShell, ScopeChips, ServerConfigPage, useInputStyle } from '../../src/features/config/ServerConfigParts';
import { Pill } from '../../src/features/config/PageHeader';
import { Segmented } from '../../src/features/config/Segmented';
import { Slider } from '../../src/features/config/Slider';
import { PROVIDERS, textOf, useServerConfig, type ServerConfig } from '../../src/features/config/serverConfig';
import { TranscriptionProviders } from '../../src/features/config/TranscriptionProviders';
import { useSettingsColors } from '../../src/features/config/colors';
import { Icon } from '../../src/ui/Icon';
import { useHandsFree } from '../../src/stores/handsFree';
import * as m from '../../src/paraglide/messages';

const ORGANIZATION_MODES = [
  { v: 'none', label: m.native_voice_organization_none, hint: m.native_voice_organization_none_hint },
  { v: 'harness', label: m.native_voice_organization_harness, hint: m.native_voice_organization_harness_hint },
  { v: 'external_api', label: m.native_voice_organization_external_api, hint: m.native_voice_organization_external_api_hint },
] as const;

type Style = 'limpar' | 'prosa' | 'briefing';
const STYLES: { v: Style; label: () => string; hint: () => string }[] = [
  { v: 'limpar', label: m.native_voice_style_limpar, hint: m.native_voice_style_limpar_hint },
  { v: 'prosa', label: m.native_voice_style_prosa, hint: m.native_voice_style_prosa_hint },
  { v: 'briefing', label: m.native_voice_style_briefing, hint: m.native_voice_style_briefing_hint },
];

/** Campos de transcrição, API de organização, briefing e leitor local. */
const SECTIONS = [['transcription_base_url', 'transcription_model'], ['llm_base_url', 'llm_api_key', 'llm_model', 'llm_reasoning_effort'],
  ['llm_briefing_base_url', 'llm_briefing_api_key', 'llm_briefing_model'], [], [], ['tts_local_cmd']];

interface Tune { key: string; label: () => string; help: () => string; left: () => string; right: () => string; def: number; min: number; max: number }
const TUNES: Tune[] = [
  { key: 'tts_stability', label: m.native_voice_tune_stability, help: m.native_voice_tune_stability_help,
    left: m.native_voice_tune_stability_left, right: m.native_voice_tune_stability_right, def: 50, min: 0, max: 100 },
  { key: 'tts_similarity_boost', label: m.native_voice_tune_similarity, help: m.native_voice_tune_similarity_help,
    left: m.native_voice_tune_similarity_left, right: m.native_voice_tune_similarity_right, def: 75, min: 0, max: 100 },
  { key: 'tts_style', label: m.native_voice_tune_style, help: m.native_voice_tune_style_help,
    left: m.native_voice_tune_style_left, right: m.native_voice_tune_style_right, def: 0, min: 0, max: 100 },
  { key: 'tts_speed', label: m.native_voice_tune_speed, help: m.native_voice_tune_speed_help,
    left: m.native_voice_tune_speed_left, right: m.native_voice_tune_speed_right, def: 100, min: 70, max: 120 },
];

const tuneValue = (cfg: ServerConfig, t: Tune) => {
  const n = Number(textOf(cfg.current(t.key)).trim() || NaN);
  return Number.isFinite(n) ? n : t.def;
};

export default function Voice() {
  const cfg = useServerConfig();
  const [open, setOpen] = useState({ transcription: false, briefing: false, eleven: false, tune: false, local: false });
  const toggle = (section: keyof typeof open) => (v: boolean) => {
    setOpen((o) => ({ ...o, [section]: v }));
    if (section === 'briefing' && !v) SECTIONS[2].forEach(cfg.unstage);
  };
  const ready = cfg.load.status === 'ready';

  // A primeira leitura boa abre as seções que já têm valor; um Salvar depois não reabre o que foi fechado.
  const openedByRead = useRef(false);
  useEffect(() => {
    if (!ready || openedByRead.current) return;
    openedByRead.current = true;
    setOpen((o) => ({ ...o, transcription: o.transcription || SECTIONS[0].some(cfg.filled), briefing: o.briefing || SECTIONS[2].some(cfg.filled) }));
  }, [ready]);

  // Seção da ElevenLabs abre quando passa a haver chave; a do leitor, quando o comando passa a ter valor.
  const elevenKey = cfg.keySet('elevenlabs_api_key');
  const localCmd = cfg.filled('tts_local_cmd');
  useEffect(() => { if (elevenKey) toggle('eleven')(true); }, [elevenKey]);
  useEffect(() => { if (localCmd) toggle('local')(true); }, [localCmd]);

  const transcribe = parseTranscriptionProviders(cfg.current(PROVIDERS)).length ? m.native_voice_status_custom()
    : cfg.keySet('groq_api_key') ? (cfg.filled('transcription_base_url') ? m.native_voice_status_custom() : m.native_voice_status_on()) : null;
  const mode = cfg.current('dictation_organization_mode');
  const organization = ORGANIZATION_MODES.find((o) => o.v === (mode == null || mode === '' ? 'none' : mode));
  const organizing = organization?.v !== 'none';
  const changeOrganization = (v: string) => {
    if (!cfg.stage('dictation_organization_mode', v)) return;
    if (v !== 'harness') ['dictation_claude_model', 'dictation_codex_model'].forEach(cfg.unstage);
    if (v !== 'external_api') [...SECTIONS[1], ...SECTIONS[2]].forEach(cfg.unstage);
    if (v === 'none') ['ditado_estilo', 'dictation_include_recent_messages'].forEach(cfg.unstage);
  };
  const read = elevenKey ? m.native_voice_status_elevenlabs() : localCmd ? m.native_voice_status_local() : null;
  const rows = (keys: string[]) => <Box>{keys.map((k) => <ConfigRow key={k} cfg={cfg} k={k} removable />)}</Box>;

  return (
    <ServerConfigPage title={m.native_settings_page_voice()} cfg={cfg}>
      <BlockHead title={m.native_voice_transcribe()} help={m.native_voice_transcribe_help()} right={<Status text={transcribe} />} />
      {!transcribe ? <Muted>{m.native_voice_transcribe_no_key()}</Muted> : null}
      {rows(['groq_api_key'])}
      <View style={styles.links}>
        <Pill icon="ExternalLink" label={m.native_voice_create_key()} onPress={() => { void Linking.openURL('https://console.groq.com/keys'); }} />
        <Disclosure open={open.transcription} label={m.native_voice_transcribe_other()} onChange={toggle('transcription')} />
      </View>
      {open.transcription ? rows(SECTIONS[0]) : null}
      <TranscriptionProviders cfg={cfg} />

      <BlockHead title={m.native_voice_after()} />
      <Box>
        <HandsFreeRow />
        <ConfigRow cfg={cfg} k="ditado_vocabulario" removable />
        <ConfigRow cfg={cfg} k="dictation_max_minutes" removable />
      </Box>

      <BlockHead title={m.native_voice_cleanup()} help={m.native_voice_cleanup_help()}
        right={<Chip text={organization?.label() ?? m.native_voice_organization_unknown()} tone={organization && organizing ? 'success' : 'muted'} />} />
      <Box>
        <RowShell icon="SlidersHorizontal" fullWidthControl
          chips={<ScopeChips cfg={cfg} k="dictation_organization_mode" />} help={organization?.hint() ?? m.native_voice_organization_unknown_hint()}>
          <Segmented options={ORGANIZATION_MODES.map((o) => ({ v: o.v, label: o.label(), aria: `${o.label()}. ${o.hint()}` }))}
            value={organization?.v ?? ''} label={m.native_voice_cleanup()} disabled={!('dictation_organization_mode' in cfg.fields)}
            onChange={changeOrganization} />
        </RowShell>
        {organizing ? <StyleRow cfg={cfg} /> : null}
        {organizing ? <RecentMessagesRow cfg={cfg} /> : null}
      </Box>
      {!organization || organization.v === 'harness' ? (
        <Box>
          <HarnessModelRow cfg={cfg} k="dictation_claude_model" label={m.native_voice_harness_model_claude()} />
          <HarnessModelRow cfg={cfg} k="dictation_codex_model" label={m.native_voice_harness_model_codex()} />
        </Box>
      ) : null}
      {!organization || organization.v === 'external_api' ? (
        <>
          {rows(SECTIONS[1])}
          <Disclosure open={open.briefing} label={m.native_voice_briefing_own()} onChange={toggle('briefing')} />
          {open.briefing ? rows(SECTIONS[2]) : null}
        </>
      ) : null}

      <BlockHead title={m.native_voice_read()} help={m.native_voice_read_help()} right={<Status text={read} />} />
      {!read ? <Muted>{m.native_voice_read_no_reader()}</Muted> : null}
      <Disclosure open={open.eleven} label={m.native_voice_elevenlabs_section()} onChange={toggle('eleven')} />
      {open.eleven ? (
        <>
          <Box>
            <ConfigRow cfg={cfg} k="elevenlabs_api_key" removable />
            {elevenKey ? <VoiceRow cfg={cfg} /> : null}
          </Box>
          {elevenKey ? <Disclosure open={open.tune} label={m.native_voice_tune()} onChange={toggle('tune')} /> : null}
          {elevenKey && open.tune ? <Box>{TUNES.map((t) => <TuneRow key={t.key} cfg={cfg} t={t} />)}</Box> : null}
        </>
      ) : null}
      <Disclosure open={open.local} label={m.native_voice_local_section()} onChange={toggle('local')} />
      {open.local ? rows(SECTIONS[5]) : null}
      {read ? rows(['tts_max_chars']) : null}
    </ServerConfigPage>
  );
}

function Status({ text }: { text: string | null }) {
  return text ? <Chip text={text} tone="success" /> : <Chip text={m.native_voice_status_off()} />;
}

/** Estilo do ditado: vai pelo rascunho, como no Rust. */
function StyleRow({ cfg }: { cfg: ServerConfig }) {
  const value = cfg.current('ditado_estilo');
  const current = ehEstilo(value) ? value : 'prosa';
  const hint = STYLES.find((s) => s.v === current)?.hint();
  return (
    <RowShell icon="Type" title={m.native_voice_style()} chips={<ScopeChips cfg={cfg} k="ditado_estilo" />} help={m.native_voice_style_help()}>
      <Segmented options={STYLES.map((s) => ({ v: s.v, label: s.label(), aria: `${s.label()}. ${s.hint()}` }))}
        value={current} label={m.native_voice_style()} disabled={!('ditado_estilo' in cfg.fields)} onChange={(v) => cfg.stage('ditado_estilo', v)} />
      {hint ? <Muted>{hint}</Muted> : null}
    </RowShell>
  );
}

// Do aparelho, não da máquina: fica fora do rascunho e grava na hora, como no PC.
function HandsFreeRow() {
  const c = useSettingsColors();
  const on = useHandsFree((s) => s.on);
  return (
    <RowShell icon="Mic" title={m.native_voice_hands_free()} help={m.native_voice_hands_free_help()}
      right={<Switch accessibilityLabel={m.native_voice_hands_free()} value={on}
        onValueChange={(next) => useHandsFree.getState().set(next)} trackColor={{ true: c.accent }} />} />
  );
}

function RecentMessagesRow({ cfg }: { cfg: ServerConfig }) {
  const c = useSettingsColors();
  const key = 'dictation_include_recent_messages';
  return (
    <RowShell icon="MessageSquare" title={m.native_voice_recent_messages()}
      chips={<ScopeChips cfg={cfg} k={key} />} help={m.native_voice_recent_messages_help()}
      right={<Switch accessibilityLabel={m.native_voice_recent_messages()} value={cfg.current(key) === true}
        disabled={!(key in cfg.fields)} onValueChange={(on) => { cfg.stage(key, on); }} trackColor={{ true: c.accent }} />} />
  );
}

function HarnessModelRow({ cfg, k, label }: { cfg: ServerConfig; k: string; label: string }) {
  const input = useInputStyle();
  return (
    <RowShell icon="Bot" title={label} chips={<ScopeChips cfg={cfg} k={k} />} help={m.native_voice_harness_model_help()}>
      <TextInput {...input} accessibilityLabel={label} editable={k in cfg.fields} autoCapitalize="none" autoCorrect={false}
        value={textOf(cfg.current(k))} onChangeText={(v) => cfg.stage(k, v)} />
    </RowShell>
  );
}

const DEFAULT_ID = '__default__';

/**
 * Voz da conta: vozes e consumo do mês só no toque, nunca ao abrir. `listarVozesTts`/`saldoTts` falam com o
 * servidor ativo, que é o desta página.
 */
function VoiceRow({ cfg }: { cfg: ServerConfig }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const key = 'elevenlabs_voice_id';
  const [voices, setVoices] = useState<TtsVoz[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [usage, setUsage] = useState<{ text: string; error: boolean } | null>(null);
  const off = !(key in cfg.fields);
  const id = textOf(cfg.current(key)).trim();
  const name = id ? voices?.find((v) => v.id === id)?.nome || id : m.native_voice_machine_default();

  const load = () => {
    if (loading) return;
    setLoading(true);
    setError('');
    setUsage(null);
    const fail = (e: unknown) => (e instanceof Error && e.message ? e.message : m.erro_desconhecido());
    listarVozesTts()
      .then((list) => setVoices(list.length ? list : null))
      .catch((e: unknown) => { setVoices(null); setError(fail(e)); })
      .finally(() => setLoading(false));
    saldoTts()
      .then((r) => setUsage({ text: m.native_voice_usage({ usados: String(r.usados ?? '?'), limite: String(r.limite ?? '?') }), error: false }))
      .catch((e: unknown) => setUsage({ text: fail(e), error: true }));
  };

  const control = voices && !error ? (
    <MenuView
      title={m.native_voice_voice()}
      actions={[{ id: DEFAULT_ID, title: m.native_voice_machine_default(), state: id ? 'off' : 'on' },
        ...voices.map((v) => ({ id: v.id, title: v.nome || v.id, state: (v.id === id ? 'on' : 'off') as 'on' | 'off' }))]}
      onPressAction={({ nativeEvent }) => { if (!off) cfg.stage(key, nativeEvent.event === DEFAULT_ID ? '' : nativeEvent.event); }}
    >
      {/* Sem Pressable aqui dentro: ele roubaria o toque que abre o menu nativo. */}
      <View accessible accessibilityRole="button" accessibilityLabel={`${m.native_voice_voice()}: ${name}`}
        style={[styles.menuPill, { borderColor: c.borderStrong }]}>
        <Text style={[styles.menuText, { color: c.text }]} numberOfLines={1}>{name}</Text>
        <Icon name="ChevronDown" size={14} color={c.muted} />
      </View>
    </MenuView>
  ) : (
    <Pill label={loading ? m.native_server_loading() : error ? m.native_server_retry() : m.native_voice_load_voices()} onPress={load} />
  );

  return (
    <RowShell icon="AudioLines" title={m.native_voice_voice()} chips={<ScopeChips cfg={cfg} k={key} />} help={m.native_voice_voice_help()}>
      <View style={styles.links}>{control}</View>
      <View style={styles.links}>
        <Text accessibilityLiveRegion="polite" style={[styles.small, { color: c.text }]}>{m.native_voice_current({ valor: name })}</Text>
        {cfg.editedInApp(key) && !cfg.removing(key) ? <Pill label={m.native_voice_back_to_default()} onPress={() => cfg.stage(key, null)} /> : null}
      </View>
      {error ? <Text accessibilityRole="alert" style={[styles.small, { color: theme.tokens.status.error }]}>{error}</Text> : null}
      {usage ? <Text style={[styles.small, { color: usage.error ? theme.tokens.status.error : c.muted }]}>{usage.text}</Text> : null}
    </RowShell>
  );
}

function TuneRow({ cfg, t }: { cfg: ServerConfig; t: Tune }) {
  const c = useSettingsColors();
  const value = tuneValue(cfg, t);
  const off = !(t.key in cfg.fields);
  return (
    <RowShell icon="SlidersHorizontal" title={`${t.label()} · ${value}`} chips={<ScopeChips cfg={cfg} k={t.key} />} help={t.help()}>
      {cfg.editedInApp(t.key) && !cfg.removing(t.key) ? (
        <View style={styles.links}><Pill label={m.native_voice_back_to_default()} onPress={() => cfg.stage(t.key, null)} /></View>
      ) : null}
      {off ? null : <Slider valor={value} min={t.min} max={t.max} label={t.label()} onChange={(v) => cfg.stage(t.key, Math.round(v))} />}
      <View style={styles.ends}>
        <Text style={[styles.small, { color: c.muted }]}>{t.left()}</Text>
        <Text style={[styles.small, styles.right, { color: c.muted }]}>{t.right()}</Text>
      </View>
    </RowShell>
  );
}

const styles = StyleSheet.create((theme) => ({
  links: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: theme.base.space[3] },
  small: { fontSize: theme.base.text.xs, lineHeight: 17 },
  ends: { flexDirection: 'row', justifyContent: 'space-between', gap: theme.base.space[3] },
  right: { textAlign: 'right' },
  menuPill: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], height: 32, paddingHorizontal: theme.base.space[3], borderWidth: 1, borderRadius: theme.base.radius.full, maxWidth: 260 },
  menuText: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium, flexShrink: 1 },
}));
