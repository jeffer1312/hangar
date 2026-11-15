import { useCallback, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { useFocusEffect } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { dreamErrorText, dreamProgressLines, dreamSince, dreamSpan, dreamUsd, type DreamProgress } from '@hangar/core';
import { SectionCard } from '../config/SectionCard';
import { useSettingsColors } from '../config/colors';
import { Icon } from '../../ui/Icon';
import { Pulse } from './rows';
import * as m from '../../paraglide/messages';

/** Relógio da tela para "há N s": só corre com a tela em foco. Não lê o servidor. */
function useNow(running: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useFocusEffect(useCallback(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]));
  return now;
}

function Button({ title, primary, disabled, onPress }: { title: string; primary?: boolean; disabled?: boolean; onPress: () => void }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <Pressable accessibilityRole="button" accessibilityLabel={title} accessibilityState={{ disabled }} disabled={disabled} onPress={onPress}
    hitSlop={6} style={[styles.button, { borderColor: primary ? c.accent : c.borderStrong, backgroundColor: primary ? c.accent : 'transparent', opacity: disabled ? 0.5 : 1 }]}>
    <Text style={[styles.buttonText, { color: primary ? theme.ink.onAccent : c.text }]}>{title}</Text>
  </Pressable>;
}

/**
 * O bloco acima de Projetos: sonhando (projeto, conversa, barra, gasto, Parar) enquanto `live` existe;
 * depois, o retrato final da rodada (`finished`): terminou ou parou.
 */
export function ProgressCard({ live, finished, busy, nextHour, onStop, onChanges, onRetry }: {
  live: DreamProgress | null; finished: DreamProgress | null; busy: boolean;
  /** Horário do sonho da noite; `null` com o sonho desligado. */
  nextHour: string | null;
  onStop: () => void; onChanges: (runId: string) => void; onRetry: () => void;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const now = useNow(!!live);
  if (live) {
    const lines = dreamProgressLines(live);
    return <SectionCard>
      <View style={styles.body}>
        <View style={styles.row}>
          <Pulse />
          <View style={styles.texts}>
            <Text style={[styles.title, { color: c.text }]}>{lines.title}</Text>
            {lines.detail ? <Text style={[styles.sub, { color: c.muted }]}>{lines.detail}</Text> : null}
          </View>
          <Button title={live.stopping ? m.dream_progress_stopping() : m.dream_progress_stop()} disabled={busy || live.stopping} onPress={onStop} />
        </View>
        <View accessibilityRole="progressbar" accessibilityValue={{ min: 0, max: 100, now: Math.round(lines.fraction * 100) }}
          style={[styles.track, { backgroundColor: c.inset }]}>
          <View style={[styles.fill, { backgroundColor: c.accent, width: `${Math.round(lines.fraction * 100)}%` }]} />
        </View>
        <Text style={[styles.foot, { color: c.faint }]}>
          {m.dream_progress_foot({ elapsed: dreamSince(live.started_at, now), cost: dreamUsd(live.spent_usd), idle: dreamSince(live.last_activity_at, now) })}
        </Text>
      </View>
    </SectionCard>;
  }
  if (!finished) return null;
  const counted = finished.projects.filter((p) => p.status !== 'skipped');
  const done = counted.filter((p) => p.status === 'done');
  const failure = counted.find((p) => p.error)?.error ?? null;
  // Nada sonhado e alguma falha: a rodada parou inteira (sem crédito, máquina fora, Parar).
  const stopped = !done.length && !!failure;
  return <SectionCard>
    <View style={[styles.body, styles.row]}>
      <Icon name={stopped ? 'X' : 'Check'} size={18} color={stopped ? theme.tokens.status.error : theme.tokens.status.success} />
      <View style={styles.texts}>
        <Text style={[styles.title, { color: c.text }]}>{stopped ? m.dream_progress_failed() : m.dream_progress_done({ ago: dreamSince(finished.last_activity_at) })}</Text>
        <Text style={[styles.sub, { color: c.muted }]}>{stopped && failure ? dreamErrorText(failure)
          : [m.dream_progress_done_sub({ done: done.length, total: counted.length, cost: dreamUsd(finished.spent_usd),
            duration: dreamSpan(Date.parse(finished.last_activity_at) - Date.parse(finished.started_at)) }),
          nextHour ? m.dream_progress_next({ hour: nextHour }) : ''].filter(Boolean).join(' · ')}</Text>
      </View>
      {stopped ? <Button title={m.dream_progress_retry()} primary disabled={busy} onPress={onRetry} />
        : <Button title={m.dream_changes()} disabled={busy} onPress={() => onChanges(finished.run_id)} />}
    </View>
  </SectionCard>;
}

const styles = StyleSheet.create((theme) => ({
  body: { paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[4] },
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  texts: { flex: 1, minWidth: 0, gap: theme.base.space[1] },
  title: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  sub: { fontSize: theme.base.text.sm },
  foot: { fontSize: theme.base.text.xs, marginTop: theme.base.space[2] },
  track: { height: 5, borderRadius: 3, overflow: 'hidden', marginTop: theme.base.space[3] },
  fill: { height: '100%', borderRadius: 3 },
  button: { minHeight: 44, minWidth: 44, paddingHorizontal: theme.base.space[4], borderRadius: 22, borderWidth: 1, justifyContent: 'center', alignItems: 'center' },
  buttonText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
}));
