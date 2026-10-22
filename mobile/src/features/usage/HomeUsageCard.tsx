import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Pressable, ScrollView, Text, View, type StyleProp, type TextStyle } from 'react-native';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withDelay, withTiming } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { mergeReports, intlLocale, localeAtual, type CostReport, type DimBucket } from '@hangar/core';
import { useSettingsColors } from '../config/colors';
import { useServers } from '../../stores/servers';
import { useReports } from './useReports';
import * as m from '../../paraglide/messages';

// Resumo das máquinas próprias na tela inicial.
type Period = 'all' | '30d' | '7d' | '1d';
type Tab = 'overview' | 'models';
type Summary = { report: Partial<CostReport>; totals: DimBucket; models: DimBucket[]; days: Map<string, number> };

// Escolhas e última leitura em memória: o cartão desmonta a cada vez que o teclado abre.
let lastPeriod: Period = 'all';
let lastTab: Tab = 'overview';
// Aparição da tela que já contou: o cartão remonta quando o teclado fecha e não deve contar de novo.
let playedEpoch = -1;

// Abertura da tela inicial: `at` é quando ela apareceu; os números e o gráfico entram depois da marca.
export type HomeIntro = { epoch: number; at: number };
const INTRO_START_MS = 420;
const COUNT_MS = 600;
const CELL_MS = 240;
const CASCADE_MS = 360;
const easeOut = Easing.out(Easing.cubic);

const raw = (b: DimBucket) => b.input + b.output + b.cache_write + b.cache_read;
const dec = (n: number, places: number) =>
  n.toLocaleString(intlLocale(), { minimumFractionDigits: places, maximumFractionDigits: places });
function tok(n: number): string {
  const en = localeAtual() === 'en';
  if (n >= 999.95e6) return `${dec(n / 1e9, 2)} ${en ? 'B' : 'Bi'}`;
  if (n >= 999.5e3) return `${dec(n / 1e6, 1)} ${en ? 'M' : 'Mi'}`;
  if (n >= 1e3) return `${dec(n / 1e3, 0)}${en ? 'k' : ' mil'}`;
  return dec(n, 0);
}
// ponytail: o app ainda não tem a preferência de moeda; fica em dólar como o Rust sem cotação.
const money = (usd: number) => `US$ ${dec(usd, 2)}`;
const br = (k: string) => k.split('-').reverse().join('/');

function summarize(report: Partial<CostReport>): Summary {
  const models = (report.by_model ?? []).filter((b) => raw(b) > 0)
    .sort((a, b) => raw(b) - raw(a) || a.key.localeCompare(b.key));
  const days = new Map<string, number>();
  for (const b of report.by_day ?? []) days.set(b.key, (days.get(b.key) ?? 0) + raw(b));
  return { report, totals: report.totals!, models, days };
}

// Semanas de segunda a domingo; a data vem do histórico, sem deslocar fuso.
function calendar(days: Map<string, number>) {
  const keys = [...days.keys()].sort();
  if (!keys.length) return null;
  const start = Date.parse(`${keys[0]}T00:00:00Z`);
  const end = Date.parse(`${keys[keys.length - 1]}T00:00:00Z`);
  if (Number.isNaN(start) || Number.isNaN(end)) return null;
  const first = start - ((new Date(start).getUTCDay() + 6) % 7) * 86400000;
  const weeks = Math.floor((end - first) / 86400000 / 7) + 1;
  const max = Math.max(1, ...days.values());
  return {
    title: m.home_usage_activity({ start: br(keys[0]), end: br(keys[keys.length - 1]) }),
    weeks: Array.from({ length: weeks }, (_, w) => Array.from({ length: 7 }, (_, d) => {
      const t = first + (w * 7 + d) * 86400000;
      const k = new Date(t).toISOString().slice(0, 10);
      const v = days.get(k) ?? 0;
      return { k, v, visible: t >= start && t <= end, level: v > 0 ? 0.3 + 0.7 * (v / max) : 0 };
    })),
  };
}

// Conta de 0 até o valor; fora da abertura mostra o valor direto. Só este texto re-renderiza.
function CountUp({ value, format, startAt, style }: {
  value: number; format: (n: number) => string; startAt: number | null; style: StyleProp<TextStyle>;
}) {
  const [shown, setShown] = useState(0);
  useEffect(() => {
    if (startAt === null) return;
    let frame = 0;
    const tick = () => {
      const t = Math.min(1, Math.max(0, (Date.now() - startAt) / COUNT_MS));
      setShown(value * (1 - (1 - t) ** 3));
      if (t < 1) frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [value, startAt]);
  return <Text style={style} numberOfLines={1}>{format(startAt === null ? value : shown)}</Text>;
}

// Uma semana do gráfico: acende (opacidade e escala) com atraso pela posição da coluna.
function Week({ startAt, delay, children }: { startAt: number | null; delay: number; children: ReactNode }) {
  const p = useSharedValue(startAt === null ? 1 : 0);
  useEffect(() => {
    if (startAt === null) { p.value = 1; return; }
    p.value = 0;
    p.value = withDelay(Math.max(0, startAt - Date.now()) + delay, withTiming(1, { duration: CELL_MS, easing: easeOut }));
  }, [startAt, delay, p]);
  const style = useAnimatedStyle(() => ({ opacity: p.value, transform: [{ scale: 0.7 + 0.3 * p.value }] }));
  return <Animated.View style={[styles.week, style]}>{children}</Animated.View>;
}

const whole = (n: number) => String(Math.round(n));

export function HomeUsageCard({ intro = null }: { intro?: HomeIntro | null }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const [period, setPeriodState] = useState<Period>(lastPeriod);
  const [tab, setTabState] = useState<Tab>(lastTab);
  const servers = useServers((s) => s.servers);
  const load = useReports('costs', servers, period, {}, true, true);
  const summary = useMemo(() => load.accepted ? summarize(mergeReports(load.parts, period).report) : null, [load.parts, load.accepted, period]);
  const loading = load.loading;
  const warming = load.parts.find((part) => part.warming)?.warming;
  const error = load.parts.filter((part) => part.error).map((part) => `${part.label}: ${part.error}`).join('\n');

  const setPeriod = (p: Period) => { lastPeriod = p; setPeriodState(p); };

  // Só a primeira leitura de cada aparição anima; troca de período, de aba ou leitura nova não.
  const played = useRef<{ epoch: number; summary: Summary | null; startAt: number }>({ epoch: -1, summary: null, startAt: 0 });
  if (intro && summary && intro.epoch !== playedEpoch && played.current.epoch !== intro.epoch) {
    played.current = { epoch: intro.epoch, summary, startAt: Math.max(intro.at + INTRO_START_MS, Date.now()) };
  }
  useEffect(() => {
    if (intro && played.current.epoch === intro.epoch) playedEpoch = intro.epoch;
  });
  const startAt = summary && played.current.summary === summary ? played.current.startAt : null;
  const setTab = (t: Tab) => { lastTab = t; played.current.summary = null; setTabState(t); };

  const muted = { color: c.muted };
  const header = (
    <View style={styles.header}>
      <TextTabs<Tab>
        label={m.home_usage_overview()}
        value={tab}
        onChange={setTab}
        options={[{ v: 'overview', label: m.home_usage_overview() }, { v: 'models', label: m.home_usage_models() }]}
      />
      <TextTabs<Period>
        label={m.home_usage_all()}
        value={period}
        onChange={setPeriod}
        options={[
          { v: 'all', label: m.home_usage_all() },
          { v: '30d', label: m.home_usage_30d() },
          { v: '7d', label: m.home_usage_7d() },
          { v: '1d', label: m.home_usage_today() },
        ]}
      />
    </View>
  );

  let body: ReactNode;
  if (loading && !summary) {
    body = (
      <Text style={[styles.note, muted]} accessibilityRole="text" accessibilityLiveRegion="polite">
        {warming ? m.home_usage_warming({ read: String(warming.read), total: String(warming.total) }) : m.native_loading()}
      </Text>
    );
  } else if (error && !summary) {
    body = (
      <View style={styles.errorBox}>
        <Text style={[styles.note, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{error}</Text>
        <Pressable accessibilityRole="button" onPress={load.refresh} hitSlop={6} style={styles.retry}>
          <Text style={[styles.retryText, { color: c.accentText }]}>{m.sync_retry()}</Text>
        </Pressable>
      </View>
    );
  } else if (!summary || (summary.totals.sessions === 0 && raw(summary.totals) === 0)) {
    body = <Text style={[styles.note, muted]}>{m.home_usage_empty()}</Text>;
  } else {
    const { totals, models, days, report } = summary;
    const partial = (report.sem_tarifa ?? []).length > 0;
    let content: ReactNode;
    if (tab === 'models') {
      const max = Math.max(1, models[0] ? raw(models[0]) : 1);
      content = models.length === 0 ? <Text style={[styles.note, muted]}>{m.home_usage_empty()}</Text> : (
        <ScrollView style={styles.modelList} contentContainerStyle={styles.modelListInner} nestedScrollEnabled>
          {models.map((b) => (
            <View key={b.key} style={styles.model}>
              <View style={styles.modelRow}>
                <Text style={[styles.modelName, { color: c.text }]} numberOfLines={1}>{b.label ?? b.key}</Text>
                <Text style={[styles.modelValue, { color: c.text }]}>{tok(raw(b))}</Text>
              </View>
              <View style={[styles.track, { backgroundColor: c.inset }]}>
                <View style={[styles.fill, { backgroundColor: c.accent, width: `${(raw(b) / max) * 100}%` }]} />
              </View>
            </View>
          ))}
        </ScrollView>
      );
    } else {
      const top = models[0] ? (models[0].label ?? models[0].key) : '—';
      const activeDays = [...days.values()].filter((v) => v > 0).length;
      const metrics: [string, number | string, (n: number) => string][] = [
        [m.home_usage_sessions(), totals.sessions, (n) => dec(n, 0)],
        [m.home_usage_tokens(), raw(totals), tok],
        [m.home_usage_cost(), totals.cost, money],
        [m.home_usage_active_days(), activeDays, whole],
        [m.home_usage_model_count(), models.length, whole],
        [m.home_usage_top_model(), top, whole],
      ];
      const cal = calendar(days);
      const step = cal ? Math.min(30, CASCADE_MS / cal.weeks.length) : 0;
      const valueStyle = [styles.metricValue, { color: c.text }];
      content = (
        <>
          {[metrics.slice(0, 3), metrics.slice(3)].map((row, i) => (
            <View key={i} style={styles.metricRow}>
              {row.map(([label, value, format]) => (
                <View key={label} style={styles.metric} accessible
                  accessibilityLabel={`${label}: ${typeof value === 'number' ? format(value) : value}`}>
                  <Text style={[styles.metricLabel, muted]} numberOfLines={1}>{label}</Text>
                  {typeof value === 'number'
                    ? <CountUp key={startAt ?? 'rest'} value={value} format={format} startAt={startAt} style={valueStyle} />
                    : <Text style={valueStyle} numberOfLines={1}>{value}</Text>}
                </View>
              ))}
            </View>
          ))}
          {cal ? (
            <View style={styles.calendar}>
              <Text style={[styles.note, muted]}>{cal.title}</Text>
              <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.weeks}>
                {cal.weeks.map((week, wi) => (
                  // A chave remonta a coluna a cada abertura: começa apagada, sem um quadro aceso antes.
                  <Week key={`${wi}-${startAt ?? 'rest'}`} startAt={startAt} delay={wi * step}>
                    {week.map((d) => (
                      <View
                        key={d.k}
                        accessible={d.visible}
                        accessibilityLabel={d.visible ? m.home_usage_day({ date: br(d.k), tokens: dec(d.v, 0) }) : undefined}
                        style={[
                          styles.cell,
                          { backgroundColor: d.v > 0 ? c.accent : c.inset, opacity: d.visible ? (d.v > 0 ? d.level : 1) : 0 },
                        ]}
                      />
                    ))}
                  </Week>
                ))}
              </ScrollView>
            </View>
          ) : null}
        </>
      );
    }
    body = (
      <>
        {content}
        <Text style={[styles.note, muted]}>
          {partial ? m.home_usage_partial() : m.home_usage_method()}
        </Text>
      </>
    );
  }

  return (
    <View
      style={styles.card}
      accessibilityLabel={m.home_usage_overview()}
    >
      {header}
      {summary && (loading || error) ? <View accessibilityLiveRegion="polite">
        <Text style={[styles.note, muted]}>{m.custos_total_parcial()}</Text>
        {load.parts.filter((part) => part.pending || part.error).map((part) => <Text key={part.id} style={[styles.note, muted]}>{`${part.label}: ${part.error ?? m.native_loading()}`}</Text>)}
        {!!error && <Pressable accessibilityRole="button" onPress={load.refresh} hitSlop={6} style={styles.retry}><Text style={[styles.retryText, { color: c.accentText }]}>{m.sync_retry()}</Text></Pressable>}
      </View> : null}
      {body}
    </View>
  );
}

/** Abas só de texto (home_usage do Rust): a escolhida ganha o fundo suave de destaque. */
export function TextTabs<T extends string>({ options, value, onChange, label }: {
  options: ReadonlyArray<{ v: T; label: string }>; value: T; onChange: (v: T) => void; label: string;
}) {
  const c = useSettingsColors();
  return (
    <View style={styles.textTabs} accessibilityRole="tablist" accessibilityLabel={label}>
      {options.map((o) => {
        const on = o.v === value;
        return (
          <Pressable key={o.v} onPress={() => onChange(o.v)} hitSlop={6} accessibilityRole="tab" accessibilityState={{ selected: on }}
            style={[styles.textTab, on && { backgroundColor: c.accentDim }]}>
            <Text style={[styles.textTabLabel, { color: on ? c.text : c.muted, fontWeight: on ? '500' : '400' }]}>{o.label}</Text>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  // Sem moldura, como no PC: os números ficam direto sobre o fundo.
  card: { width: '100%', maxWidth: 560, alignSelf: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[1] },
  header: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2], justifyContent: 'space-between', alignItems: 'center' },
  textTabs: { flexDirection: 'row', gap: theme.base.space[0.5] },
  textTab: { height: 28, paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm, justifyContent: 'center' },
  textTabLabel: { fontSize: theme.base.text.sm },
  note: { fontSize: theme.base.text.xs },
  errorBox: { gap: theme.base.space[2], alignItems: 'flex-start' },
  retry: { minHeight: 36, justifyContent: 'center' },
  retryText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  metricRow: { flexDirection: 'row', gap: theme.base.space[3] },
  metric: { flex: 1, minWidth: 0, gap: theme.base.space[1] },
  metricLabel: { fontSize: theme.base.text.xs },
  metricValue: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium, fontVariant: ['tabular-nums'] },
  calendar: { gap: theme.base.space[2] },
  // flexGrow centraliza quando cabe; quando não cabe o conteúdo passa da largura e rola.
  weeks: { gap: theme.base.space[0.5], flexGrow: 1, justifyContent: 'center' },
  week: { gap: theme.base.space[0.5] },
  cell: { width: 11, height: 11, borderRadius: theme.base.radius.xs },
  modelList: { maxHeight: 224 },
  modelListInner: { gap: theme.base.space[2] },
  model: { gap: theme.base.space[1] },
  modelRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  modelName: { flex: 1, minWidth: 0, fontSize: theme.base.text.sm },
  modelValue: { fontSize: theme.base.text.sm, fontVariant: ['tabular-nums'] },
  track: { height: 4, borderRadius: theme.base.radius.full, overflow: 'hidden' },
  fill: { height: '100%', borderRadius: theme.base.radius.full },
}));
