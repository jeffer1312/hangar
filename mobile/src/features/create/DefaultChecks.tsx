import { useEffect, useMemo, useRef, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { applySessionDefault, applyToOwnServers, machineLabel, ownUpdatableServers, saveSessionDefaultsForServer,
  setClaudeDefaultsForServer, type ApplyResult, type Provider, type Server, type SessionDefaults,
  type SessionDefaultWrite } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { Icon } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { loadErrorText } from './loadError';
import * as m from '../../paraglide/messages';

/** O escopo do padrão de criação: agente, motor e conta do motor; `key` muda junto com eles. */
export interface DefaultScope { key: string; provider: Provider; engine: string | null; engineAccount: string | null }
type Line = { serverId: string; status: ApplyResult['status'] | 'pending'; error?: string };

const same = (a: SessionDefaults, b: SessionDefaults) => a.model === b.model && a.effort === b.effort && a.permission === b.permission;

/**
 * As duas caixas do menu de modelo do PC: marcar grava a escolha de agora como padrão de criação do
 * servidor (desmarcar apaga) e, no Claude sem motor, também no settings.json da máquina; "Em todas as
 * minhas máquinas" repete a mesma gravação nas outras máquinas próprias. Fica fora do painel para a
 * gravação e o resultado sobreviverem a fechá-lo.
 */
export function useDefaultChecks({ server, scope, choice, saved, onSaved }: {
  server: Server; scope: DefaultScope; choice: SessionDefaults; saved: SessionDefaults | null;
  onSaved: (key: string, value: SessionDefaults | null) => void;
}) {
  const servers = useServers((s) => s.servers);
  const others = useMemo(() => ownUpdatableServers(servers).filter((s) => s.id !== server.id), [servers, server.id]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [spreadOn, setSpreadOn] = useState(false);
  const [spread, setSpread] = useState<{ write: SessionDefaultWrite; lines: Line[] } | null>(null);
  const saveSeq = useRef(0);
  const spreadSeq = useRef(0);
  // Uma gravação nas outras máquinas por vez: cada máquina termina com a última escolha.
  const spreadChain = useRef<Promise<unknown>>(Promise.resolve());
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  // O resultado é do escopo em que foi gravado: outro agente ou motor começa sem ele.
  useEffect(() => { spreadSeq.current++; setSpread(null); setError(''); }, [scope.key]);

  const checked = !!saved && same(saved, choice);
  const writeOf = (value: SessionDefaults | null): SessionDefaultWrite => ({
    provider: scope.provider, engine: scope.engine, engineAccount: scope.engineAccount, value,
    mirror: scope.provider === 'claude' && !scope.engine,
  });

  const runSpread = async (write: SessionDefaultWrite, only?: Server[]) => {
    const targets = only ?? others;
    if (!targets.length) return;
    const seq = only ? spreadSeq.current : ++spreadSeq.current;
    setSpread((cur) => ({ write, lines: only && cur
      ? cur.lines.map((l): Line => targets.some((t) => t.id === l.serverId) ? { serverId: l.serverId, status: 'pending' } : l)
      : targets.map((t): Line => ({ serverId: t.id, status: 'pending' })) }));
    const run = spreadChain.current.then(() => applyToOwnServers(targets, (s) => applySessionDefault(s, write)));
    spreadChain.current = run.catch(() => undefined);
    const fresh = await run;
    if (!alive.current || seq !== spreadSeq.current) return;
    setSpread((cur) => cur ? { ...cur, lines: cur.lines.map((l) => fresh.find((f) => f.serverId === l.serverId) ?? l) } : cur);
    if (fresh.every((r) => r.status === 'ok' || r.status === 'absent')) haptic.success(); else haptic.error();
  };

  const toggle = async () => {
    if (saving) return;
    const value = checked ? null : { ...choice };
    const key = scope.key;
    const seq = ++saveSeq.current;
    setSaving(true);
    setError('');
    if (spreadOn) void runSpread(writeOf(value));
    try {
      const result = await saveSessionDefaultsForServer(server, scope.provider, value, scope.engine, scope.engineAccount);
      // Gravação mais nova já respondeu ou vai responder: esta não decide a caixa.
      if (!alive.current || seq !== saveSeq.current) return;
      onSaved(key, result);
      if (value && scope.provider === 'claude' && !scope.engine) await setClaudeDefaultsForServer(server, value);
    } catch (cause: unknown) {
      if (alive.current && seq === saveSeq.current) {
        setError(m.native_create_default_failed({ erro: loadErrorText(cause, m.criar_sessao_erro()) }));
        haptic.error();
      }
    } finally {
      if (alive.current && seq === saveSeq.current) setSaving(false);
    }
  };

  const toggleSpread = () => {
    const on = !spreadOn;
    setSpreadOn(on);
    spreadSeq.current++;
    setSpread(null);
    // Marcar repete na hora o padrão que já vale aqui; sem ele, a linha de espera pede marcar o de cima.
    if (on && checked && saved) void runSpread(writeOf(saved));
  };

  const retry = (serverId: string) => {
    const target = others.filter((s) => s.id === serverId);
    if (spread && target.length) void runSpread(spread.write, target);
  };

  return { checked, saving, error, others, spreadOn, spread, toggle, toggleSpread, retry };
}

const STATUS_TEXT: Record<ApplyResult['status'], (error?: string) => string> = {
  ok: () => m.tags_apply_ok(),
  offline: () => m.tags_apply_offline(),
  old: () => m.tags_apply_old(),
  absent: () => m.tags_apply_absent(),
  failed: (error) => m.tags_apply_failed({ reason: error ?? '' }),
};

export function DefaultChecks({ control, providerLabel, ready, disabled = false }: {
  control: ReturnType<typeof useDefaultChecks>; providerLabel: string; ready: boolean; disabled?: boolean;
}) {
  const { theme } = useUnistyles();
  const { others, spread, spreadOn } = control;
  // Marcada sem padrão igual à escolha, não há o que enviar; calada, pareceria aplicada.
  const waiting = spreadOn && !spread && !control.checked;
  return <View style={styles.root}>
    <Check label={m.native_create_default_for_harness({ harness: providerLabel })} checked={control.checked}
      busy={control.saving} disabled={disabled || control.saving || !ready} onPress={() => void control.toggle()} />
    {control.error ? <Text style={[styles.note, styles.danger]} accessibilityRole="alert">{control.error}</Text> : null}
    {others.length ? <>
      <Check label={m.tags_apply_all()} checked={spreadOn} disabled={disabled} onPress={control.toggleSpread} />
      {spread?.lines.map((line) => {
        const server = others.find((s) => s.id === line.serverId);
        if (!server) return null;
        const bad = line.status === 'offline' || line.status === 'failed';
        const text = line.status === 'pending' ? m.comum_carregando() : STATUS_TEXT[line.status](line.error);
        const color = line.status === 'ok' ? theme.tokens.status.success : bad ? theme.tokens.status.error : theme.tokens.text.muted;
        return <View key={line.serverId} style={styles.line}>
          <Text style={[styles.note, styles.grow]} numberOfLines={2} accessibilityRole="text">
            <Text>{`${machineLabel(server)}: `}</Text><Text style={{ color }}>{text}</Text>
          </Text>
          {bad ? <Pressable accessibilityRole="button" accessibilityLabel={`${m.tags_apply_retry()}, ${machineLabel(server)}`}
            disabled={disabled} onPress={() => control.retry(line.serverId)} hitSlop={{ top: 6, bottom: 6 }} style={styles.retry}>
            <Text style={styles.retryText}>{m.tags_apply_retry()}</Text>
          </Pressable> : null}
        </View>;
      })}
      {waiting ? <Text style={styles.note} accessibilityRole="text">{m.tags_apply_needs_default()}</Text> : null}
    </> : null}
  </View>;
}

function Check({ label, checked, disabled, busy = false, onPress }: {
  label: string; checked: boolean; disabled: boolean; busy?: boolean; onPress: () => void;
}) {
  const { theme } = useUnistyles();
  return <Pressable accessibilityRole="checkbox" accessibilityLabel={label} accessibilityState={{ checked, disabled, busy }}
    disabled={disabled} onPress={() => { haptic.selection(); onPress(); }}
    style={({ pressed }) => [styles.check, pressed && { backgroundColor: theme.tokens.fillSubtle }, disabled && styles.off]}>
    <View style={[styles.box, checked ? { backgroundColor: theme.tokens.accent.base, borderColor: theme.tokens.accent.base }
      : { borderColor: theme.tokens.border.default }]}>
      {checked ? <Icon name="Check" size={13} color={theme.ink.onAccent} /> : null}
    </View>
    <Text style={styles.label} numberOfLines={2}>{label}</Text>
  </Pressable>;
}

const styles = StyleSheet.create((theme) => ({
  root: { gap: theme.base.space[0.5], paddingVertical: theme.base.space[1] },
  check: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm },
  box: { width: 18, height: 18, borderRadius: theme.base.radius.xs, borderWidth: 1, alignItems: 'center', justifyContent: 'center' },
  label: { flex: 1, minWidth: 0, fontSize: theme.base.text.xs, color: theme.tokens.text.primary },
  off: { opacity: 0.5 },
  line: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingLeft: theme.base.space[6] },
  grow: { flex: 1, minWidth: 0 },
  note: { paddingHorizontal: theme.base.space[2], fontSize: theme.base.text.xxs, color: theme.tokens.text.muted },
  danger: { color: theme.tokens.status.error },
  retry: { minHeight: 32, justifyContent: 'center', paddingHorizontal: theme.base.space[2] },
  retryText: { fontSize: theme.base.text.xxs, fontWeight: theme.base.weight.medium, color: theme.tokens.accent.base },
}));
