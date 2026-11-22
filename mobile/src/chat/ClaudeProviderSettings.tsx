import { useEffect, useRef, useState } from 'react';
import { Text } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import {
  claudeProviderModels, getEnginesForServer, switchClaudeProvider,
  type ClaudeProviderSource, type ClaudeProviderTarget, type EnginesResponse, type ModelOption, type Server,
} from '@hangar/core';
import { SettingsList } from '../ui/SettingsList';
import { SessionSettingsLink } from './SessionSettingsFrame';
import * as m from '../paraglide/messages';

type Row = {
  label: string;
  hint: string;
  current: boolean;
  engine: string;
  target: ClaudeProviderTarget | null;
};

export function useClaudeProviderEngines(server: Server | null, enabled: boolean) {
  const [result, setResult] = useState<{ server: Server; inventory?: EnginesResponse; failed?: boolean }>();
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!enabled || !server) return;
    let alive = true;
    setResult(undefined);
    void getEnginesForServer(server).then(
      inventory => {
        if (alive) setResult(inventory.claude_provider_switch && inventory.arquivo_corrompido ? { server, failed: true } : { server, inventory });
      },
      () => { if (alive) setResult({ server, failed: true }); },
    );
    return () => { alive = false; };
  }, [server, enabled, retry]);
  return { result: result?.server === server ? result : undefined, reload: () => setRetry(n => n + 1) };
}

function providerRows(inventory: EnginesResponse, source: ClaudeProviderSource): Row[] {
  return Object.entries(inventory.motores).filter(([, motor]) => !!motor.cliproxy_binding).map(([engine, motor]) => {
    const bound = motor.cliproxy_bound_account;
    const binding = motor.cliproxy_binding ?? '';
    const enabled = !!bound?.account && !motor.cliproxy_error && !motor.cliproxy_binding_error;
    return { engine, label: motor.label || engine, current: source.engine === engine && source.engine_binding === binding,
      hint: enabled && bound ? (bound.label || bound.email) : m.native_claude_provider_unavailable(),
      target: enabled && bound ? { engine, engine_account: bound.account, engine_binding: binding } : null };
  }).sort((a, b) => a.label.localeCompare(b.label) || a.engine.localeCompare(b.engine));
}

export function ClaudeProviderSettings({ server, name, source, ready, inventory, onApplied, onCancel, onBusy, reloadProviders }: {
  server: Server;
  name: string;
  source: ClaudeProviderSource;
  ready: boolean;
  inventory: EnginesResponse;
  onApplied: (model: string, origin: ClaudeProviderSource) => void;
  onCancel: () => void;
  onBusy: (busy: boolean) => void;
  reloadProviders: () => void;
}) {
  const [origin] = useState(source);
  const unchanged = origin.lifecycle_id === source.lifecycle_id && origin.jsonl === source.jsonl && origin.headless === source.headless
    && origin.engine === source.engine && origin.engine_account === source.engine_account && origin.engine_binding === source.engine_binding;
  const [choice, setChoice] = useState<Row | null>(null);
  const [catalog, setCatalog] = useState<ModelOption[] | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [review, setReview] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [catalogFailed, setCatalogFailed] = useState(false);
  const [pending, setPending] = useState(false);
  const load = useRef<AbortController | null>(null);
  const apply = useRef(false);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; load.current?.abort(); };
  }, []);

  const rows = providerRows(inventory, source);
  const loadModels = (row: Row) => {
    if (!row.target || apply.current) return;
    load.current?.abort();
    const controller = new AbortController();
    load.current = controller;
    setChoice(row); setModel(null); setCatalog(null); setError(null); setCatalogFailed(false);
    void claudeProviderModels(server, row.target, controller.signal).then(
      models => { if (!controller.signal.aborted) setCatalog(models); },
      (reason: unknown) => {
        if (controller.signal.aborted) return;
        setCatalogFailed(true);
        setError(reason instanceof Error && reason.message ? reason.message : m.native_claude_provider_read_failed());
      },
    );
  };
  const matching = rows.find(row => row.engine === choice?.engine && row.target?.engine_binding === choice?.target?.engine_binding
    && row.target?.engine_account === choice?.target?.engine_account);
  const canSubmit = !!matching?.target && !!model && !!catalog?.some(row => row.id === model) && ready && unchanged && !pending;
  const reload = () => {
    load.current?.abort();
    setChoice(null); setModel(null); setCatalog(null); setCatalogFailed(false); setReview(false); setError(null);
    reloadProviders();
  };
  const confirm = async () => {
    if (apply.current || !review || !canSubmit || !matching?.target || !model) return;
    apply.current = true;
    onBusy(true); setPending(true); setError(null);
    try {
      const result = await switchClaudeProvider(server, name, { ...matching.target, model, source: origin });
      if (!result.ok || result.engine !== matching.target.engine || result.engine_account !== matching.target.engine_account
        || result.engine_binding !== matching.target.engine_binding || result.model !== model || result.service_tier !== 'default') {
        setError(m.native_claude_provider_changed());
        return;
      }
      if (mounted.current) onApplied(result.model, origin);
    } catch (reason) {
      if (mounted.current) setError(reason instanceof Error && reason.message ? reason.message : m.native_claude_provider_changed());
    } finally {
      apply.current = false;
      onBusy(false);
      if (mounted.current) setPending(false);
    }
  };

  if (review && choice && model) {
    return <>
      <ProviderConfirmationSummary account={`${choice.label} · ${choice.hint}`} model={model} />
      {!unchanged && !pending ? <Text style={styles.error}>{m.native_claude_provider_changed()}</Text> : null}
      {error ? <Text style={styles.error} accessibilityRole="alert">{error}</Text> : null}
      <SessionSettingsLink label={m.native_claude_provider_back()} disabled={pending} onPress={() => { setReview(false); setError(null); }} />
      <SessionSettingsLink label={m.comum_cancelar()} disabled={pending} onPress={onCancel} />
      <SessionSettingsLink label={pending ? m.comum_carregando() : m.native_claude_provider_confirm()} disabled={!canSubmit} onPress={() => void confirm()} />
    </>;
  }
  return <>
    <SessionSettingsLink label={m.native_claude_provider_back()} onPress={onCancel} />
    <Text accessibilityRole="header" style={styles.title}>{m.native_claude_provider_choose_account()}</Text>
    {rows.length ? <SettingsList comfortable searchable={false} items={rows.map(row => ({ id: row.engine, label: row.label,
      hint: row.current ? `${row.hint} · ${m.native_claude_provider_current()}` : row.hint,
      disabled: !row.target, selected: choice?.engine === row.engine }))}
      onPick={id => { const row = rows.find(item => item.engine === id); if (row) loadModels(row); }} />
      : <Text style={styles.note}>{m.native_claude_provider_empty()}</Text>}
    {choice ? <>
      <Text accessibilityRole="header" style={styles.title}>{m.native_claude_provider_models({ name: choice.label })}</Text>
      {catalogFailed ? <>
        <Text accessibilityRole="alert" style={styles.error}>{error}</Text>
        <SessionSettingsLink label={m.native_voice_models_refresh()} onPress={reload} />
      </> : !catalog ? <Text style={styles.note}>{m.comum_carregando()}</Text> : catalog.length ?
        <SettingsList comfortable searchable="always" items={catalog.map(row => ({ id: row.id, label: row.name || row.id, selected: row.id === model }))}
          onPick={setModel} /> : <Text style={styles.note}>{m.native_claude_provider_no_models()}</Text>}
    </> : null}
    {!ready ? <Text style={styles.note}>{m.native_claude_provider_busy()}</Text> : null}
    <SessionSettingsLink label={m.native_claude_provider_apply()} disabled={!canSubmit} onPress={() => { setReview(true); setError(null); }} />
  </>;
}

export function ProviderConfirmationSummary({ account, model }: { account: string; model: string }) {
  return <>
    <Text accessibilityRole="header" style={styles.title}>{m.native_claude_provider_confirm_title()}</Text>
    <Text style={styles.note}>{m.native_claude_provider_summary({ account, model })}</Text>
    <Text style={styles.note}>{m.native_claude_provider_history()}</Text>
  </>;
}

const styles = StyleSheet.create(theme => ({
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  note: { fontSize: theme.base.text.xs, color: theme.tokens.text.secondary },
  error: { fontSize: theme.base.text.xs, color: theme.tokens.status.error },
}));
