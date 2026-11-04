import { useEffect, useMemo, useRef, useState } from 'react';
import { Pressable, Switch, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { applyCodexContext, applyToOwnServers, codexOpcoes, machineLabel, ownUpdatableServers,
  type ApplyResult, type Server } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

const STATUS_TEXT: Record<ApplyResult['status'], (error?: string) => string> = {
  ok: () => m.tags_apply_ok(),
  offline: () => m.tags_apply_offline(),
  old: () => m.tags_apply_old(),
  absent: () => m.tags_apply_absent(),
  failed: (error) => m.tags_apply_failed({ reason: error ?? '' }),
};

// As outras máquinas próprias: a desta tela já recebe a gravação do interruptor.
function ApplyEverywhere({ here, value, disabled }: { here: string | null; value: boolean; disabled: boolean }) {
  const servers = useServers((s) => s.servers);
  const own = useMemo(() => ownUpdatableServers(servers).filter((s) => s.id !== here), [servers, here]);
  const [results, setResults] = useState<ApplyResult[] | null>(null);
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  // O resultado é do valor aplicado; trocar o interruptor o torna velho.
  useEffect(() => { setResults(null); }, [value]);
  if (own.length === 0) return null;
  const run = async (targets: Server[]) => {
    setBusy(true);
    const fresh = await applyToOwnServers(targets, (server) => applyCodexContext(server, value));
    if (!alive.current) return;
    setResults((prev) => prev ? prev.map((r) => fresh.find((f) => f.serverId === r.serverId) ?? r) : fresh);
    setBusy(false);
    if (fresh.every((r) => r.status === 'ok' || r.status === 'absent')) haptic.success(); else haptic.error();
  };
  return (
    <View style={styles.root}>
      <Pressable accessibilityRole="button" accessibilityState={{ disabled: disabled || busy, busy }} disabled={disabled || busy}
        style={[styles.retry, (disabled || busy) && styles.disabled]} onPress={() => void run(own)}>
        <Text style={styles.action}>{busy ? m.tags_saving() : m.tags_apply_all()}</Text>
      </Pressable>
      {results?.map((r) => {
        const server = own.find((s) => s.id === r.serverId);
        if (!server) return null;
        const bad = r.status === 'offline' || r.status === 'failed';
        return (
          <View key={r.serverId} style={styles.row}>
            <Text style={[styles.hint, styles.result, bad && styles.errorText]} numberOfLines={2}>
              {`${machineLabel(server)}: ${STATUS_TEXT[r.status](r.error)}`}
            </Text>
            {bad ? <Pressable accessibilityRole="button" accessibilityLabel={`${m.tags_apply_retry()}, ${machineLabel(server)}`}
              disabled={busy} style={styles.retry} onPress={() => void run([server])}>
              <Text style={styles.action}>{m.tags_apply_retry()}</Text>
            </Pressable> : null}
          </View>
        );
      })}
    </View>
  );
}

export function useCodexContextControl(server: Server | null, open: boolean) {
  const [enabled, setEnabled] = useState(false);
  const [pedido, setPedido] = useState<boolean | null>(null);
  const [ready, setReady] = useState(false);
  const [reading, setReading] = useState(false);
  const [writing, setWriting] = useState(false);
  const [error, setError] = useState('');
  const [retry, setRetry] = useState(0);
  const readController = useRef<AbortController | null>(null);
  const writePending = useRef(false);
  const serverKey = server ? `${server.id}\0${server.baseUrl}\0${server.token}` : '';
  const currentKey = useRef(serverKey);
  currentKey.current = serverKey;

  useEffect(() => {
    setEnabled(false);
    setPedido(null);
    setReady(false);
    setError('');
  }, [serverKey]);

  useEffect(() => {
    if (!open || !server || ready || writePending.current) return;
    const target = { ...server };
    const key = serverKey;
    const controller = new AbortController();
    readController.current = controller;
    setReading(true);
    setError('');
    void codexOpcoes(target, controller.signal)
      .then((result) => {
        if (controller.signal.aborted || currentKey.current !== key) return;
        setEnabled(result.contexto_estendido);
        setReady(true);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted && currentKey.current === key) {
          setError(cause instanceof Error ? cause.message : m.comum_falha_aplicar());
        }
      })
      .finally(() => {
        if (readController.current === controller) { readController.current = null; setReading(false); }
      });
    return () => {
      if (readController.current === controller) {
        controller.abort();
        readController.current = null;
        setReading(false);
      }
    };
  }, [server, serverKey, open, ready, writing, retry]);

  const select = async (value: boolean) => {
    if (!server || !ready || writePending.current) return;
    const target = { ...server };
    const key = serverKey;
    writePending.current = true;
    setWriting(true);
    setPedido(value);
    setError('');
    // Fechar o painel não cancela uma escrita global que o servidor já recebeu.
    const controller = new AbortController();
    try {
      const result = await codexOpcoes(target, controller.signal, value);
      if (currentKey.current !== key) return;
      setEnabled(result.contexto_estendido);
      setReady(true);
    } catch (cause: unknown) {
      if (currentKey.current === key) setError(cause instanceof Error ? cause.message : m.comum_falha_aplicar());
    } finally {
      writePending.current = false;
      setWriting(false);
      setPedido(null);
    }
  };

  return { server, value: pedido ?? enabled, ready, busy: reading || writing, saving: writing, error, select,
    retry: () => { if (!writePending.current) { setReady(false); setRetry((n) => n + 1); } } };
}

export function CodexContextControl({ control }: { control: ReturnType<typeof useCodexContextControl> }) {
  return (
    <View style={styles.root}>
      <View style={styles.row}>
        <Text style={styles.label}>{m.codex_contexto_titulo()}</Text>
        <Switch accessibilityLabel={m.codex_contexto_titulo()} value={control.value} disabled={control.busy || !control.ready}
          onValueChange={(value) => void control.select(value)} />
      </View>
      <Text style={styles.hint}>{m.codex_contexto_padrao()}</Text>
      <ApplyEverywhere here={control.server?.id ?? null} value={control.value} disabled={control.busy || !control.ready} />
      {control.busy ? <Text style={styles.hint} accessibilityRole="text">
        {control.saving ? m.codex_contexto_salvando() : m.comum_carregando()}</Text> : null}
      {control.error ? <>
        <Text style={styles.error} accessibilityRole="alert">{control.error}</Text>
        <Pressable accessibilityRole="button" disabled={control.busy} style={styles.retry} onPress={control.retry}>
          <Text style={styles.label}>{m.config_server_tentar_de_novo()}</Text>
        </Pressable>
      </> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: { gap: theme.base.space[2] },
  row: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', minHeight: 44, gap: theme.base.space[3] },
  label: { color: theme.tokens.text.primary, flexShrink: 1 },
  hint: { color: theme.tokens.text.secondary, fontSize: theme.base.text.xs },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.xs },
  retry: { minHeight: 44, justifyContent: 'center' },
  action: { color: theme.tokens.accent.base, fontSize: theme.base.text.sm },
  disabled: { opacity: 0.5 },
  result: { flex: 1 },
  errorText: { color: theme.tokens.status.error },
}));
