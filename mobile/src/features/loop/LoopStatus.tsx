import { useState } from 'react';
import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { loopBadge } from '@hangar/core';
import type { LoopState } from '@hangar/core';
import * as m from '../../paraglide/messages';

interface Props {
  loop: LoopState;
  stopBusy: boolean;
  resolveBusy: boolean;
  stopError: string;
  onStop: () => void;
  onResolve: (accept: boolean) => void;
  onNew: () => void;
}

export function LoopStatus({ loop, stopBusy, resolveBusy, stopError, onStop, onResolve, onNew }: Props) {
  const { theme } = useUnistyles();
  const [expandedHist, setExpandedHist] = useState<number | null>(null);
  const badge = loopBadge(loop.status, loop.iter, loop.max_iters);
  const toneColor = badge
    ? {
        ok: theme.tokens.accent.base,
        warn: theme.tokens.status.error,
        attention: theme.tokens.status.warning,
        muted: theme.tokens.text.muted,
      }[badge.tone]
    : theme.tokens.text.muted;
  const statusLabel: Record<LoopState['status'], string> = {
    running: m.atividade_rodando(),
    paused_awaiting: m.loop_estado_aguardando(),
    done_claimed: m.loop_estado_pronto_confirmacao(),
    done: m.atividade_concluido(),
    stopped: m.loop_estado_parado(),
    exhausted: m.loop_estado_esgotou(),
    failed: m.preview_falhou(),
  };
  const final = loop.status === 'done' || loop.status === 'stopped' || loop.status === 'exhausted' || loop.status === 'failed';

  return (
    <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.loop_titulo()}</Text>
      <View style={styles.statusRow}>
        <View style={[styles.dot, { backgroundColor: toneColor }]} />
        <Text style={[styles.statusLabel, { color: theme.tokens.text.primary }]}>{statusLabel[loop.status]}</Text>
        <Text style={[styles.iter, { color: theme.tokens.text.muted }]}>{loop.iter}/{loop.max_iters}</Text>
      </View>
      <Text style={[styles.goal, { color: theme.tokens.text.secondary }]} selectable>{loop.goal}</Text>

      {loop.status === 'done_claimed' ? (
        <View style={[styles.claim, { backgroundColor: theme.tokens.accent.dim }]}>
          <Text style={[styles.claimText, { color: theme.tokens.text.primary }]}>{m.loop_terminou_confirma()}</Text>
          <View style={styles.claimActions}>
            <Pressable
              onPress={() => onResolve(true)}
              disabled={resolveBusy}
              style={[styles.primary, { backgroundColor: theme.tokens.accent.base }, resolveBusy && styles.disabled]}
              accessibilityRole="button"
            >
              <Text style={styles.primaryText}>{m.loop_confirmar_pronto()}</Text>
            </Pressable>
            <Pressable
              onPress={() => onResolve(false)}
              disabled={resolveBusy}
              style={[styles.ghost, { borderColor: theme.tokens.border.default }, resolveBusy && styles.disabled]}
              accessibilityRole="button"
            >
              <Text style={[styles.ghostText, { color: theme.tokens.text.secondary }]}>{m.loop_rejeitar()}</Text>
            </Pressable>
          </View>
        </View>
      ) : final ? (
        <>
          {loop.ended_reason ? <Text style={[styles.reason, { color: theme.tokens.text.muted }]} selectable>{loop.ended_reason}</Text> : null}
          <Pressable onPress={onNew} style={[styles.primary, { backgroundColor: theme.tokens.accent.base }]} accessibilityRole="button">
            <Text style={styles.primaryText}>{m.loop_novo()}</Text>
          </Pressable>
        </>
      ) : null}

      {loop.history.length ? (
        <View style={styles.history}>
          {loop.history.map((entry) => {
            const expanded = expandedHist === entry.n;
            return (
              <View key={entry.n} style={[styles.historyRow, { borderBottomColor: theme.tokens.border.subtle }]}>
                <Pressable
                  onPress={() => setExpandedHist(expanded ? null : entry.n)}
                  style={styles.historyLine}
                  accessibilityRole="button"
                  accessibilityState={{ expanded }}
                >
                  <Text style={[styles.historyText, { color: theme.tokens.text.secondary }]} numberOfLines={1}>
                    {entry.n} · {m.ctx_saida()} {entry.check_exit ?? '—'} · {entry.tail.split('\n')[0] ?? ''}
                  </Text>
                </Pressable>
                {expanded ? (
                  <Text style={[styles.historyTail, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle, color: theme.tokens.text.muted }]} selectable>
                    {entry.tail}
                  </Text>
                ) : null}
              </View>
            );
          })}
        </View>
      ) : null}

      {stopError ? (
        <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert" selectable>
          {stopError}
        </Text>
      ) : null}

      {!final && loop.status !== 'done_claimed' ? (
        <Pressable
          onPress={onStop}
          disabled={stopBusy}
          style={[styles.ghost, { borderColor: theme.tokens.border.default }, stopBusy && styles.disabled]}
          accessibilityRole="button"
        >
          <Text style={[styles.ghostText, { color: theme.tokens.status.error }]}>{m.loop_parar()}</Text>
        </Pressable>
      ) : null}
    </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  scroll: {
    padding: theme.base.space[4],
    gap: theme.base.space[3],
    paddingBottom: 40,
  },
  title: {
    fontSize: 20,
    fontWeight: '600',
  },
  statusRow: {
    minHeight: 28,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
  },
  dot: {
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  statusLabel: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
  iter: {
    marginLeft: 'auto',
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.xs,
  },
  goal: {
    fontSize: theme.base.text.sm,
    lineHeight: 21,
  },
  claim: {
    gap: theme.base.space[2],
    padding: theme.base.space[3],
    borderRadius: theme.base.radius.md,
  },
  claimText: {
    fontSize: theme.base.text.sm,
  },
  claimActions: {
    flexDirection: 'row',
    gap: theme.base.space[2],
  },
  primary: {
    minHeight: 50,
    flex: 1,
    borderRadius: theme.base.radius.md,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[3],
  },
  primaryText: {
    color: '#fff',
    fontSize: theme.base.text.sm,
    fontWeight: '600',
    textAlign: 'center',
  },
  ghost: {
    minHeight: 44,
    flex: 1,
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[3],
  },
  ghostText: {
    fontSize: theme.base.text.sm,
    fontWeight: '500',
    textAlign: 'center',
  },
  disabled: {
    opacity: 0.5,
  },
  reason: {
    fontSize: theme.base.text.sm,
    lineHeight: 21,
  },
  history: {
    gap: theme.base.space[1],
  },
  historyRow: {
    borderBottomWidth: 1,
  },
  historyLine: {
    minHeight: 44,
    justifyContent: 'center',
  },
  historyText: {
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.xs,
  },
  historyTail: {
    marginBottom: theme.base.space[2],
    padding: theme.base.space[2],
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.xs,
  },
  error: {
    fontSize: theme.base.text.sm,
  },
}));
