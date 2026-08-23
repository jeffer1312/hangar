import { useEffect, useState } from 'react';
import { ActivityIndicator, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useLocalSearchParams } from 'expo-router';
import { getLimits, resetsIn } from '@hangar/core';
import type { SessionLimits, RateLimitWindow } from '@hangar/core';
import * as m from '../../../../src/paraglide/messages';

export default function CodexLimitsSheet() {
  const { theme } = useUnistyles();
  const { server, name } = useLocalSearchParams<{ server: string; name: string }>();
  const sessionName = String(name ?? '');
  const [limits, setLimits] = useState<SessionLimits | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    getLimits(sessionName)
      .then((res) => {
        if (!cancelled) setLimits(res);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionName]);

  function label(w: RateLimitWindow): string {
    const pct = w.usedPercent != null ? `${Math.round(w.usedPercent)}%` : '—';
    const inTxt = resetsIn(w.resetsAt);
    const reset = inTxt ? ` · reseta ${inTxt}` : '';
    return m.codex_limite_uso({ pct, reset });
  }

  return (
    <View style={[styles.container, { backgroundColor: theme.tokens.bg.base }]}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.codex_limites_titulo()}</Text>
      {server ? <Text style={[styles.subtitle, { color: theme.tokens.text.muted }]}>{String(server)} / {String(name)}</Text> : null}
      {loading ? (
        <View style={styles.center}>
          <ActivityIndicator color={theme.tokens.text.secondary} />
          <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
        </View>
      ) : error ? (
        <Text style={[styles.err, { color: theme.tokens.status.error }]}>{error}</Text>
      ) : !limits ? (
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.codex_sem_dados_limite()}</Text>
      ) : !limits.primary && !limits.secondary ? (
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.codex_sem_dados_limite()}</Text>
      ) : (
        <ScrollView contentContainerStyle={styles.list}>
          {limits.primary ? (
            <View style={[styles.row, { backgroundColor: theme.tokens.bg.surface }]}>
              <Text style={[styles.rowLabel, { color: theme.tokens.text.primary }]}>{m.codex_limite_principal()}</Text>
              <Text style={[styles.rowValue, { color: theme.tokens.text.secondary }]}>{label(limits.primary)}</Text>
            </View>
          ) : null}
          {limits.secondary ? (
            <View style={[styles.row, { backgroundColor: theme.tokens.bg.surface }]}>
              <Text style={[styles.rowLabel, { color: theme.tokens.text.primary }]}>{m.codex_limite_secundario()}</Text>
              <Text style={[styles.rowValue, { color: theme.tokens.text.secondary }]}>{label(limits.secondary)}</Text>
            </View>
          ) : null}
          {limits.planType ? (
            <Text style={[styles.plan, { color: theme.tokens.text.muted }]}>{m.codex_plano({ plano: limits.planType })}</Text>
          ) : null}
        </ScrollView>
      )}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  container: {
    flex: 1,
    padding: theme.base.space[4],
    gap: theme.base.space[3],
  },
  title: {
    fontSize: theme.base.text.lg,
    fontWeight: '600',
  },
  subtitle: {
    fontSize: theme.base.text.xs,
  },
  center: {
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingVertical: theme.base.space[4],
  },
  muted: {
    fontSize: theme.base.text.sm,
    textAlign: 'center',
  },
  err: {
    fontSize: theme.base.text.sm,
    textAlign: 'center',
  },
  list: {
    gap: theme.base.space[2],
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    padding: theme.base.space[3],
    borderRadius: theme.base.radius.md,
    gap: theme.base.space[3],
  },
  rowLabel: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
  rowValue: {
    fontSize: theme.base.text.sm,
  },
  plan: {
    fontSize: theme.base.text.xs,
    marginTop: theme.base.space[2],
  },
}));
