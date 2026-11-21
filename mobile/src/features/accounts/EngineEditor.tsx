import { useEffect, useRef, useState } from 'react';
import { ScrollView, Switch, Text, TextInput, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { engineCliproxyForServer, putEngineForServer, type Motor, type Server } from '@hangar/core';
import { Sheet } from '../../ui/Sheet';
import { PageHeader, Pill } from '../config/PageHeader';
import { SettingsRow } from '../config/SettingsRow';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

const flags = ['bundled_skills', 'experimental_betas', 'prompt_caching', 'adaptive_thinking', 'tool_search',
  'gateway_model_discovery', 'fine_grained_tool_streaming', 'auth_via_api_key'] as const;
const numbers = ['context_window', 'auto_compact_window', 'max_output_tokens'] as const;
type Flag = typeof flags[number];
type NumberField = typeof numbers[number];
type TextField = 'name' | 'label' | 'base_url' | 'api_key' | 'model' | 'subagent_model' | NumberField;

export function EngineEditor({ server, name, engine, names, onSaved, onClose, onCliproxy }: {
  server: Server; name?: string; engine?: Motor; names: string[]; onSaved: () => void; onClose: () => void;
  /** Com ele, "usar CLIProxy" troca este formulário pelo assistente de instalação e login. */
  onCliproxy?: () => void;
}) {
  const c = useSettingsColors();
  const [text, setText] = useState<Record<TextField, string>>(() => ({
    name: name ?? '', label: engine?.label ?? name ?? '', base_url: engine?.base_url ?? '', api_key: '',
    model: engine?.model ?? '', subagent_model: engine?.subagent_model ?? '',
    context_window: engine?.context_window?.toString() ?? '', auto_compact_window: engine?.auto_compact_window?.toString() ?? '',
    max_output_tokens: engine?.max_output_tokens?.toString() ?? '',
  }));
  const [enabled, setEnabled] = useState<Record<Flag, boolean>>(() => Object.fromEntries(flags.map(flag => [flag,
    engine?.[flag] ?? (flag === 'prompt_caching' || flag === 'adaptive_thinking'),
  ])) as Record<Flag, boolean>);
  const [vision, setVision] = useState<boolean | null>(engine?.vision ?? null);
  const [binding, setBinding] = useState<string | null | undefined>(engine?.cliproxy_binding);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [proxyUrl, setProxyUrl] = useState<string | null>(null);
  const [proxyNotice, setProxyNotice] = useState('');
  const [detecting, setDetecting] = useState(false);
  const detectingNow = useRef(false);
  const saving = useRef(false);
  const locked = busy || detecting;
  const alive = useRef(true);
  const identity = JSON.stringify([server.id, server.baseUrl, server.token]);
  const currentIdentity = useRef(identity);
  const openingIdentity = useRef(identity);
  const closed = useRef(false);
  const closeNotified = useRef(false);
  currentIdentity.current = identity;
  if (identity !== openingIdentity.current) closed.current = true;
  function close() {
    closed.current = true;
    if (!closeNotified.current) { closeNotified.current = true; onClose(); }
  }
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  useEffect(() => { if (closed.current) close(); }, [identity, onClose]);

  const labels: Record<TextField | Flag, string> = {
    name: m.mobile_accounts_engine_name(), label: m.mobile_accounts_engine_label(), base_url: m.mobile_accounts_engine_base_url(),
    api_key: m.mobile_accounts_engine_key(), model: m.mobile_accounts_engine_model(), subagent_model: m.mobile_accounts_engine_subagent_model(),
    context_window: m.mobile_accounts_engine_context_window(), auto_compact_window: m.mobile_accounts_engine_auto_compact_window(),
    max_output_tokens: m.mobile_accounts_engine_max_output_tokens(), bundled_skills: m.mobile_accounts_engine_bundled_skills(),
    experimental_betas: m.mobile_accounts_engine_experimental_betas(), prompt_caching: m.mobile_accounts_engine_prompt_caching(),
    adaptive_thinking: m.mobile_accounts_engine_adaptive_thinking(), tool_search: m.mobile_accounts_engine_tool_search(),
    gateway_model_discovery: m.mobile_accounts_engine_gateway_model_discovery(), fine_grained_tool_streaming: m.mobile_accounts_engine_fine_grained_tool_streaming(),
    auth_via_api_key: m.mobile_accounts_engine_auth_via_api_key(),
  };
  async function detectProxy() {
    if (name || proxyUrl || detectingNow.current || saving.current || !alive.current || closed.current) return;
    const target = server;
    const requestIdentity = identity;
    const valid = () => alive.current && !closed.current && currentIdentity.current === requestIdentity;
    detectingNow.current = true; setDetecting(true); setError(''); setProxyNotice('');
    try {
      const result = await engineCliproxyForServer(target);
      if (!valid()) return;
      if (result.found && result.base_url && result.error === null) {
        const url = result.base_url;
        setProxyUrl(url);
        setText(prev => ({ ...prev, base_url: url, api_key: '' }));
        setEnabled(prev => ({ ...prev, experimental_betas: true }));
        setProxyNotice(m.cliproxy_found({ url }));
      } else if (result.error) {
        setError(result.base_url ? m.cliproxy_error({ url: result.base_url, erro: result.error }) : result.error);
      } else setProxyNotice(m.cliproxy_missing());
    } catch (cause) {
      if (valid()) setError(cause instanceof Error ? cause.message : m.erro_desconhecido());
    } finally {
      detectingNow.current = false;
      if (valid()) setDetecting(false);
    }
  }
  async function save() {
    if (saving.current || detectingNow.current || !alive.current || closed.current) return;
    const value = Object.fromEntries(Object.entries(text).map(([key, value]) => [key, value.trim()])) as Record<TextField, string>;
    const id = name ?? value.name;
    const baseUrl = proxyUrl ?? value.base_url;
    if (!id || !value.label || !baseUrl || !value.model || (!proxyUrl && !engine?.api_key_definida && !value.api_key)) {
      setError(m.mobile_accounts_engine_required()); return;
    }
    if (!/^[a-z0-9][a-z0-9_-]{0,31}$/.test(id)) { setError(m.mobile_accounts_engine_bad_name()); return; }
    if (!name && names.includes(id)) { setError(m.mobile_accounts_engine_name_taken()); return; }
    try {
      const url = new URL(baseUrl);
      if (!['http:', 'https:'].includes(url.protocol) || !url.hostname || url.username || url.password) throw new Error();
    } catch { setError(m.mobile_accounts_engine_bad_url()); return; }
    if (numbers.some(key => value[key] && (!/^\d+$/.test(value[key]) || !Number.isSafeInteger(Number(value[key])) || Number(value[key]) <= 0))) {
      setError(m.mobile_accounts_engine_bad_number()); return;
    }
    const payload: Record<string, unknown> = { label: value.label, base_url: baseUrl, model: value.model,
      subagent_model: value.subagent_model, ...enabled };
    if (binding !== engine?.cliproxy_binding) payload.cliproxy_binding = binding;
    if (vision !== null) payload.vision = vision;
    for (const key of numbers) payload[key] = value[key] ? Number(value[key]) : '';
    if (proxyUrl) payload.use_cliproxy_key = true;
    else if (value.api_key) payload.api_key = value.api_key;
    const target = server;
    const requestIdentity = identity;
    const valid = () => alive.current && !closed.current && currentIdentity.current === requestIdentity;
    saving.current = true; setBusy(true); setError('');
    try {
      await putEngineForServer(target, id, payload);
      if (valid()) onSaved();
    } catch (cause) {
      if (valid()) setError(cause instanceof Error ? cause.message : m.erro_desconhecido());
    } finally {
      saving.current = false;
      if (valid()) setBusy(false);
    }
  }
  const field = (key: TextField) => (
    <SettingsRow key={key} title={labels[key]}
      description={key === 'api_key' && engine?.api_key_definida ? m.mobile_accounts_engine_key_saved() : undefined}>
      <TextInput accessibilityLabel={labels[key]} value={text[key]} onChangeText={value => {
        if (proxyUrl && (key === 'base_url' || key === 'api_key')) return;
        setText(prev => ({ ...prev, [key]: value }));
      }}
        editable={!locked && !(key === 'name' && !!name) && !(proxyUrl && (key === 'base_url' || key === 'api_key'))} secureTextEntry={key === 'api_key'}
        autoCapitalize="none" autoCorrect={false} keyboardType={numbers.includes(key as NumberField) ? 'number-pad' : key === 'base_url' ? 'url' : 'default'}
        placeholderTextColor={c.faint} style={[styles.input, { color: c.text, borderColor: c.borderStrong, backgroundColor: c.inset }]} />
    </SettingsRow>
  );
  if (closed.current) return null;
  return (
    <Sheet open sizes={['large']} scrollable dismissible={!locked} onDismiss={close}>
      <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={styles.body}>
        <PageHeader title={name ? m.mobile_accounts_engine_edit() : m.mobile_accounts_engine_new()} subtitle={server.label}
          actions={[{ label: m.comum_cancelar(), disabled: locked, onPress: close }]} />
        {!name ? <View style={styles.actions}>
          <Pill label={m.browser_jev_aux_cliproxy_use()} disabled={locked || proxyUrl !== null}
            onPress={() => { if (onCliproxy) onCliproxy(); else void detectProxy(); }} />
        </View> : null}
        {detecting || proxyNotice ? <Text style={styles.muted}>{detecting ? m.cliproxy_detecting() : proxyNotice}</Text> : null}
        {(['name', 'label', 'base_url', 'api_key', 'model', 'subagent_model', ...numbers] as TextField[]).map(field)}
        {engine && (engine.cliproxy_accounts || engine.cliproxy_binding) ? <SettingsRow title={m.cliproxy_bound_account()}
          description={engine.cliproxy_binding_error || (!binding ? m.cliproxy_no_bound_account() : undefined)}>
          <View style={styles.actions}>
            {(engine.cliproxy_accounts ?? []).filter(account => !!account.binding && !!account.prefix).map(account => (
              <Pill key={account.binding} label={[account.plan, account.prefix, account.email || account.label].filter(Boolean).join(' · ')} disabled={locked || account.binding === binding}
                onPress={() => setBinding(account.binding)} />
            ))}
            {binding ? <Pill label={m.cliproxy_unbind_account()} disabled={locked} onPress={() => setBinding(null)} /> : null}
          </View>
        </SettingsRow> : null}
        <View style={styles.actions}>
          {([null, true, false] as const).filter(value => value !== null || engine?.vision == null).map(value => <Pill key={String(value)}
            label={value == null ? m.mobile_accounts_engine_vision_auto() : value ? m.mobile_accounts_engine_vision_yes() : m.mobile_accounts_engine_vision_no()}
            disabled={locked || value === vision} onPress={() => setVision(value)} />)}
        </View>
        {flags.map(flag => <SettingsRow key={flag} title={labels[flag]}>
          <Switch accessibilityLabel={labels[flag]} value={enabled[flag]} disabled={locked}
            onValueChange={value => setEnabled(prev => ({ ...prev, [flag]: value }))} />
        </SettingsRow>)}
        {name && engine?.api_key_definida && !text.api_key.trim() && text.base_url.trim() !== engine.base_url
          ? <Text style={styles.muted}>{m.native_accounts_engine_url_moved()}</Text> : null}
        {error ? <Text accessibilityRole="alert" style={styles.error}>{error}</Text> : null}
        {busy ? <Text style={styles.muted}>{m.comum_carregando()}</Text> : null}
        <View style={styles.actions}><Pill label={m.mobile_accounts_engine_save()} disabled={locked} onPress={() => { void save(); }} /></View>
      </ScrollView>
    </Sheet>
  );
}

const styles = StyleSheet.create(theme => ({
  body: { padding: theme.base.space[4], paddingBottom: theme.base.space[8], gap: theme.base.space[3] },
  input: { minHeight: 44, borderWidth: 1, borderRadius: theme.base.radius.sm, paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], fontSize: theme.base.text.sm },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.xs },
  muted: { color: theme.tokens.text.muted, fontSize: theme.base.text.xs },
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2] },
}));
