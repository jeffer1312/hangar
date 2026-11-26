import { useMemo } from 'react';
import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { parseStatusLine } from '@hangar/core';
import { Icon } from '../ui/Icon';
import { chatStore } from '../stores/chat';
import { useSessions } from '../stores/sessions';
import { ContextRing } from './ContextRing';
import { statusChips } from './statusChips';
import { linhaStats, settingsLabel, usageWindows } from './usage';
import { useAgyUsageWindows } from './useAgyUsageWindows';
import { superficie } from '../theme/superficie';
import * as m from '../paraglide/messages';

interface Props {
  serverId: string;
  name: string;
  onOpenLimits?: () => void;
  showTitle?: boolean;
}

const clamp = (n: number) => Math.min(100, Math.max(0, n));

// Lê só o que o SSE já trouxe; sem dado, mostra o vazio.
export function UsagePanel({ serverId, name, onOpenLimits, showTitle = true }: Props) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const chat = chatStore(serverId, name);
  const statusLine = chat.use((s) => s.statusLine);
  const stats = chat.use((s) => s.stats);
  const turnState = chat.use((s) => s.stateEvent?.state);
  const session = useSessions((s) => s.byServerRecord?.[serverId]?.find((x) => x.name === name)
    ?? s.rows.find((x) => x.serverId === serverId && x.name === name) ?? null);
  const f = useMemo(() => parseStatusLine(statusLine, session), [statusLine, session]);
  const chips = useMemo(() => statusChips(f), [f]);
  const agyWindows = useAgyUsageWindows(serverId, session, turnState === 'idle');
  const windows = useMemo(() => { const own = usageWindows(f); return own.length ? own : agyWindows; }, [f, agyWindows]);
  const numbers = stats ? linhaStats(stats) : [];
  const isCodex = session?.provider === 'codex';

  // Mesmos limiares do anel (70 / 90): a cor do medidor diz o mesmo que o anel do composer.
  const tone = (pct: number) => (pct >= 90 ? theme.tokens.status.error : pct >= 70 ? theme.tokens.status.warning : theme.tokens.accent.base);
  const chip = (key: string) => chips.find((c) => c.key === key)?.text;
  const rows: { label: string; value: string }[] = [];
  if (chip('cost')) rows.push({ label: m.uso_custo(), value: chip('cost')! });
  if (f?.sessionTime) rows.push({ label: m.uso_tempo_sessao(), value: f.sessionTime });
  if (f?.model) rows.push({ label: m.composer_modelo(), value: settingsLabel(f.model, f.effort) });
  if (f?.repo || f?.branch) rows.push({ label: m.uso_linha_projeto(), value: [f.repo, f.branch].filter(Boolean).join(' · ') });
  const ctxPct = f?.ctxPct;
  const hasCtx = typeof ctxPct === 'number' && isFinite(ctxPct);
  const ctxTokens = f?.ctxUsed != null ? `${f.ctxUsed.toLocaleString()}${f.ctxTotal ? ` / ${f.ctxTotal.toLocaleString()}` : ''}` : '';
  // Statusline sem marcador reconhecido continua visível crua, como a faixa antiga fazia.
  const parsed = hasCtx || rows.length > 0 || windows.length > 0;
  const empty = !parsed && !numbers.length && !statusLine;

  const meter = (label: string, pct: number, sub?: string) => (
    <View style={styles.meter} key={label}>
      <View style={styles.meterHead}>
        <Text style={[styles.label, { color: theme.tokens.text.primary }]}>{label}</Text>
        <Text style={[styles.pct, { color: pct >= 70 ? tone(pct) : theme.tokens.text.primary }]}>{Math.round(clamp(pct))}%</Text>
      </View>
      <View style={[styles.bar, { backgroundColor: superficie(theme, 0.8) }]}>
        <View style={[styles.fill, { width: `${clamp(pct)}%`, backgroundColor: tone(pct) }]} />
      </View>
      {sub ? <Text style={[styles.sub, { color: theme.tokens.text.muted }]}>{sub}</Text> : null}
    </View>
  );

  return (
      <ScrollView contentContainerStyle={styles.inner} accessibilityLabel={m.uso_aria()}>
        {showTitle ? <Text style={[styles.title, { color: theme.tokens.text.primary }]} accessibilityRole="header">{m.uso_titulo()}</Text> : null}

        {empty ? <Text style={[styles.sub, { color: theme.tokens.text.muted }]}>{m.uso_vazio()}</Text> : null}

        {windows.length ? (
          <View style={styles.section}>
            <Text style={[styles.sectionTitle, { color: theme.tokens.text.muted }]}>{m.uso_secao_cota()}</Text>
            {windows.map((w) => meter(w.label, w.pct, w.reset ? m.uso_reset({ quando: w.reset }) : undefined))}
          </View>
        ) : null}

        {hasCtx || rows.length ? (
          <View style={styles.section}>
            <Text style={[styles.sectionTitle, { color: theme.tokens.text.muted }]}>{m.uso_secao_conversa()}</Text>
            {hasCtx ? (
              <View style={styles.ctxRow}>
                <ContextRing pct={ctxPct} size={40} />
                <View style={styles.ctxMeter}>{meter(m.ctx_contexto(), ctxPct!, ctxTokens || undefined)}</View>
              </View>
            ) : null}
            {rows.map((r) => (
              <View key={r.label} style={styles.row}>
                <Text style={[styles.label, { color: theme.tokens.text.secondary }]}>{r.label}</Text>
                <Text style={[styles.value, { color: theme.tokens.text.primary }]} numberOfLines={2}>{r.value}</Text>
              </View>
            ))}
          </View>
        ) : null}

        {numbers.length ? (
          <View style={styles.section}>
            <Text style={[styles.sectionTitle, { color: theme.tokens.text.muted }]}>{m.uso_secao_numeros()}</Text>
            <View style={styles.numbers} accessibilityLabel={m.stats_faixa_aria()}>
              {numbers.map((n) => (
                <Text key={n} style={[styles.number, { color: theme.tokens.text.primary, backgroundColor: superficie(theme, 0.8), borderColor: theme.tokens.border.subtle }]}>
                  {n}
                </Text>
              ))}
            </View>
          </View>
        ) : null}

        {!parsed && statusLine ? (
          <View style={styles.section}>
            <Text style={[styles.sectionTitle, { color: theme.tokens.text.muted }]}>{m.uso_statusline()}</Text>
            <Text style={[styles.raw, { color: theme.tokens.text.secondary }]} selectable>{statusLine}</Text>
          </View>
        ) : null}

        {isCodex ? (
          <Pressable
            onPress={() => {
              if (onOpenLimits) onOpenLimits();
              else router.push(`/s/${serverId}/${name}/codex-limits` as never);
            }}
            style={({ pressed }) => [styles.link, { borderColor: theme.tokens.border.subtle }, pressed && { backgroundColor: theme.tokens.bg.hover }]}
            accessibilityRole="button"
            accessibilityLabel={m.codex_limites_titulo()}
          >
            <Icon name="Gauge" size={18} color={theme.tokens.text.secondary} />
            <Text style={[styles.linkText, { color: theme.tokens.text.primary }]}>{m.codex_limites_titulo()}</Text>
            <Icon name="ChevronRight" size={16} color={theme.tokens.text.muted} />
          </Pressable>
        ) : null}
      </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: {
    padding: theme.base.space[4],
    gap: theme.base.space[5],
  },
  title: {
    fontSize: theme.base.text.lg,
    fontWeight: theme.base.weight.semibold,
  },
  section: {
    gap: theme.base.space[3],
  },
  sectionTitle: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.semibold,
  },
  meter: {
    gap: theme.base.space[1.5],
  },
  meterHead: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    gap: theme.base.space[3],
  },
  bar: {
    height: 6,
    borderRadius: theme.base.radius.full,
    overflow: 'hidden',
  },
  fill: {
    height: '100%',
    minWidth: 2,
    borderRadius: theme.base.radius.full,
  },
  ctxRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
  },
  ctxMeter: {
    flex: 1,
  },
  row: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'flex-start',
    gap: theme.base.space[3],
  },
  label: {
    fontSize: theme.base.text.sm,
  },
  value: {
    flexShrink: 1,
    textAlign: 'right',
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.medium,
  },
  pct: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
  },
  sub: {
    fontSize: theme.base.text.xs,
  },
  numbers: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: theme.base.space[2],
  },
  number: {
    fontSize: theme.base.text.xs,
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    overflow: 'hidden',
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1],
  },
  raw: {
    fontSize: theme.base.text.xs,
    fontFamily: theme.base.fontMono,
  },
  link: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    minHeight: 48,
    paddingHorizontal: theme.base.space[3],
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
  },
  linkText: {
    flex: 1,
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
  },
}));
