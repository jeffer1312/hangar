import { ActivityIndicator, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { UpdateAllCheck } from '@hangar/core';
import { useSettingsColors } from '../config/colors';
import { Icon } from '../../ui/Icon';
import * as m from '../../paraglide/messages';

export type RowTone = 'busy' | 'waiting' | 'ok' | 'warn' | 'error';

export function checkText(check: UpdateAllCheck): string {
  switch (check.status) {
    case 'consultando': return m.update_all_checking();
    case 'disponivel': return m.update_all_available();
    case 'em_dia': return m.update_all_up_to_date();
    case 'fora_do_ar': return m.update_all_offline_skip();
    case 'falhou': return m.update_all_failed({ erro: check.error || m.erro_desconhecido() });
    case 'travado':
      switch (check.reason) {
        case 'branch': return m.update_all_blocked_branch();
        case 'mudancas': return m.update_all_blocked_changes();
        case 'rodando': return m.update_all_blocked_running();
        default: return m.update_all_blocked_missing();
      }
  }
}

export function checkTone(check: UpdateAllCheck): RowTone {
  switch (check.status) {
    case 'consultando': return 'busy';
    case 'disponivel': case 'em_dia': return 'ok';
    case 'falhou': return 'error';
    default: return 'warn';
  }
}

/** A máquina e como anda: ícone de estado, nome e a frase dele. */
export function UpdateRow({ label, text, tone, detail }: { label: string; text: string; tone: RowTone; detail?: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const color = tone === 'error' ? theme.tokens.status.error : tone === 'warn' ? theme.tokens.status.warning : undefined;
  return <View accessible accessibilityLabel={`${label}, ${text}`} style={styles.row}>
    <View style={styles.icon}>
      {tone === 'busy' ? <ActivityIndicator size="small" color={c.muted} />
        : tone === 'ok' ? <Icon name="CircleCheck" size={18} color={theme.tokens.status.success} />
        : tone === 'waiting' ? <Icon name="Circle" size={18} color={c.faint} />
        : <Icon name={tone === 'error' ? 'CircleAlert' : 'TriangleAlert'} size={18} color={color} />}
    </View>
    <View style={styles.text}>
      <Text style={[styles.label, { color: c.text }]}>{label}</Text>
      <Text selectable style={[styles.status, { color: color ?? c.muted }]}>{text}</Text>
      {detail ? <Text selectable style={[styles.status, { color: c.muted }]}>{detail}</Text> : null}
    </View>
  </View>;
}

/** A frase do conjunto e a barra fina: quanto da rodada já terminou. */
export function UpdateSummary({ text, progress }: { text: string; progress: number }) {
  const c = useSettingsColors();
  const share = Math.max(0, Math.min(1, progress));
  return <View style={styles.summary}>
    <Text accessibilityLiveRegion="polite" style={[styles.status, { color: c.muted }]}>{text}</Text>
    <View accessibilityRole="progressbar" accessibilityValue={{ min: 0, max: 100, now: Math.round(share * 100) }}
      style={[styles.bar, { backgroundColor: c.borderStrong }]}>
      <View style={[styles.fill, { width: `${share * 100}%`, backgroundColor: c.accent }]} />
    </View>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  row: { minHeight: 44, flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[3], paddingVertical: theme.base.space[2] },
  icon: { width: 20, height: 20, marginTop: 1, alignItems: 'center', justifyContent: 'center' },
  text: { flex: 1, gap: theme.base.space[0.5] },
  label: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  status: { fontSize: theme.base.text.sm },
  summary: { gap: theme.base.space[2] },
  bar: { height: 4, borderRadius: theme.base.radius.full, overflow: 'hidden' },
  fill: { height: '100%', borderRadius: theme.base.radius.full },
}));
