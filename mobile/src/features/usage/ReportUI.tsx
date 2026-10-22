import { Children, Fragment as ReactFragment, useMemo, useState, type ReactNode } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { create } from 'zustand';
import Svg, { Circle, Line, Polyline, Rect } from 'react-native-svg';
import { intlLocale, type Server } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { Stack, useRouter } from 'expo-router';
import { HeaderBack, useGlassHeader } from '../../ui/glassHeader';
import { Pagina } from '../config/Pagina';
import { Sheet } from '../../ui/Sheet';
import { Icon, type IconName } from '../../ui/Icon';
import { useSettingsColors } from '../config/colors';
import type { ReportPart } from './useReports';
import * as m from '../../paraglide/messages';
import { RADIUS, SPACE } from '../../theme/scale';

export const useUsagePreferences = create(() => ({ period: '30d', off: [] as string[] }));
export const number = (value: number, digits = 0) => value.toLocaleString(intlLocale(), { maximumFractionDigits: digits });
export const compactNumber = (value: number) => value.toLocaleString(intlLocale(), { notation: 'compact', maximumFractionDigits: 1 });
export const dollars = (value: number) => `US$ ${value.toLocaleString(intlLocale(), { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
export const percent = (part: number, total: number) => `${number(total > 0 ? part / total * 100 : 0, 1)}%`;

export function useReportSelection() {
  const servers = useServers((s) => s.servers);
  const period = useUsagePreferences((s) => s.period);
  const off = useUsagePreferences((s) => s.off);
  const own = servers.filter((s) => !s.invite && !s.disabled && !s.ephemeral);
  return { period, own, selected: own.filter((s) => !off.includes(s.id)) };
}

export function Copy({ children, title = false, muted = false }: { children: ReactNode; title?: boolean; muted?: boolean }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  return <Text style={{ color: muted ? c.muted : c.text, fontSize: title ? theme.base.text.lg : theme.base.text.sm, fontWeight: title ? '600' : '400' }}>{children}</Text>;
}

export function Action({ label, accessibilityLabel, onPress, selected = false, disabled = false, icon, iconOnly = false }: { label: string; accessibilityLabel?: string; onPress: () => void; selected?: boolean; disabled?: boolean; icon?: IconName; iconOnly?: boolean }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  return <Pressable accessibilityRole="button" accessibilityLabel={accessibilityLabel ?? label} accessibilityState={{ selected, disabled }} disabled={disabled} onPress={onPress}
    style={{ minHeight: 44, ...(iconOnly ? { minWidth: 44 } : {}), paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderRadius: theme.base.radius.md, backgroundColor: selected ? c.accentDim : 'transparent', flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], opacity: disabled ? 0.45 : 1 }}>
    {icon && <Icon name={icon} size={18} color={c.accentText} />}
    {!iconOnly && <Text style={{ color: selected ? c.accentText : c.text, fontSize: theme.base.text.sm, flexShrink: 1 }}>{label}</Text>}
  </Pressable>;
}

export function Card({ children, flat = false }: { children: ReactNode; flat?: boolean }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View style={{ padding: flat ? 0 : theme.base.space[4], paddingVertical: theme.base.space[4], gap: theme.base.space[2], backgroundColor: flat ? 'transparent' : c.inset, borderRadius: flat ? 0 : theme.base.radius.lg, ...(flat ? { borderTopWidth: 1, borderColor: c.borderStrong } : {}) }}>{children}</View>;
}

export function Metric({ label, value, primary = false, children }: { label: string; value: string; primary?: boolean; children?: ReactNode }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  return <View style={{ gap: SPACE[1.5], paddingVertical: SPACE[3], minWidth: 0 }}>
    <Copy muted>{label}</Copy>
    <Text style={{ color: c.text, fontSize: primary ? theme.base.text.xl : theme.base.text.lg, fontWeight: '600', fontVariant: ['tabular-nums'] }}>{value}</Text>
    {children}
  </View>;
}

export function Grid({ children }: { children: ReactNode }) {
  return <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: SPACE[2] }}>{Children.toArray(children).map((child, index) => <View key={index} style={{ flexGrow: 1, flexBasis: '45%', minWidth: 140 }}>{child}</View>)}</View>;
}

export function Search({ value, onChange }: { value: string; onChange: (text: string) => void }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <TextInput accessibilityLabel={m.uso_busca()} placeholder={m.uso_busca()} placeholderTextColor={c.muted} value={value} onChangeText={onChange}
    style={{ color: c.text, fontSize: theme.base.text.sm, padding: theme.base.space[3], minHeight: 44, borderWidth: 1, borderColor: c.borderStrong, borderRadius: theme.base.radius.md }} />;
}

export function Paged<T>({ items, render }: { items: T[]; render: (item: T, index: number) => ReactNode }) {
  const [limit, setLimit] = useState(20);
  if (!items.length) return <Copy muted>{m.uso_vazio_secao()}</Copy>;
  return <View style={{ gap: SPACE[2] }}>{items.slice(0, limit).map((item, index) => <View key={index}>{render(item, index)}</View>)}
    {items.length > limit && <Action label={m.uso_mostrar_mais({ n: items.length - limit })} onPress={() => setLimit((n) => n + 20)} />}
    {limit > 20 && <Action label={m.uso_mostrar_menos()} onPress={() => setLimit(20)} />}
  </View>;
}

export function Choice({ label, options, values, onChange, max, closeOnSelect = false, triggerLabel }: { label: string; options: { key: string; label?: string | null }[]; values: string[]; onChange: (values: string[]) => void; max?: number; closeOnSelect?: boolean; triggerLabel?: string }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const all = [...options];
  for (const key of values) if (!all.some((item) => item.key === key)) all.push({ key });
  const matches = all.filter((item) => `${item.key} ${item.label ?? ''}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  return <>
    <Action label={triggerLabel ?? label} accessibilityLabel={label} icon={triggerLabel ? 'ChevronDown' : 'ListFilter'} selected={values.length > 0} onPress={() => setOpen(true)} />
    {!triggerLabel && values.length > 0 && <Copy muted>{values.map((key) => all.find((item) => item.key === key)?.label ?? key).join(' · ')}</Copy>}
    {open && <Sheet open onDismiss={() => setOpen(false)} sizes={['large']} scrollable>
      <ScrollView contentContainerStyle={{ padding: SPACE[4], gap: SPACE[2] }} keyboardShouldPersistTaps="handled">
        <Copy title>{label}</Copy>
        <Search value={query} onChange={setQuery} />
        <Action label={m.uso_limpar()} onPress={() => onChange([])} />
        <Paged key={query} items={matches} render={(item) => <Action label={item.label ?? item.key} accessibilityLabel={`${label}: ${item.label ?? item.key}`} selected={values.includes(item.key)}
          disabled={!values.includes(item.key) && max !== undefined && max > 1 && values.length >= max}
          icon={values.includes(item.key) ? 'CheckSquare' : 'Square'} onPress={() => { if (closeOnSelect) setOpen(false); onChange(values.includes(item.key) ? values.filter((v) => v !== item.key) : max === 1 ? [item.key] : [...values, item.key]); }} />} />
        <Action label={m.native_close()} icon="X" onPress={() => setOpen(false)} />
      </ScrollView>
    </Sheet>}
  </>;
}

export function ReportFrame({ title, back, sibling, onSibling, children, compact = false }: { title: string; back: () => void; sibling: string; onSibling: () => void; children: ReactNode; compact?: boolean }) {
  const router = useRouter();
  const header = useGlassHeader();
  const canGoBack = router.canGoBack();
  const headerSibling = compact ? onSibling : undefined;
  const headerOptions = useMemo(() => ({ ...header, title, headerLeft: canGoBack ? undefined : () => <HeaderBack onPress={back} />,
    ...(headerSibling ? { headerRight: () => <Action label={sibling} icon="ChartNoAxesCombined" iconOnly onPress={headerSibling} /> } : {}),
  }), [header, title, canGoBack, back, headerSibling, sibling]);
  return <>
    <Stack.Screen options={headerOptions} />
    <Pagina>
      {compact ? <View style={{ gap: SPACE[2] }}>{children}</View> : <><Action label={sibling} icon="ChartNoAxesCombined" onPress={onSibling} />{children}</>}
    </Pagina>
  </>;
}

export function ReportControls({ own, refresh }: { own: Server[]; refresh: () => void }) {
  const period = useUsagePreferences((s) => s.period);
  const off = useUsagePreferences((s) => s.off);
  const periods = [['1d', m.custos_periodo_1d()], ['7d', m.custos_periodo_7d()], ['30d', m.custos_periodo_30d()], ['90d', m.custos_periodo_90d()], ['all', m.custos_periodo_tudo()]];
  return <Card><Copy title>{m.custos_periodo()}</Copy>
    <View style={{ flexDirection: 'row', flexWrap: 'wrap' }}>{periods.map(([key, label]) => <Action key={key} label={label} selected={period === key} onPress={() => useUsagePreferences.setState({ period: key })} />)}</View>
    <Choice label={m.custos_servidores_relatorio()} options={own.map((s) => ({ key: s.id, label: s.label }))} values={own.filter((s) => !off.includes(s.id)).map((s) => s.id)} onChange={(chosen) => useUsagePreferences.setState({ off: own.filter((s) => !chosen.includes(s.id)).map((s) => s.id) })} />
    <Action label={m.custos_atualizar()} icon="RefreshCw" onPress={refresh} />
  </Card>;
}

export function ReportStatus<R>({ parts, loading, accepted, selected }: { parts: ReportPart<R>[]; loading: boolean; accepted: boolean; selected: number }) {
  return <View accessibilityLiveRegion="polite" style={{ gap: SPACE[2] }}>
    {loading && <><ActivityIndicator /><Copy>{m.custos_carregando_maquinas({ n: parts.filter((p) => p.pending).length || selected })}</Copy></>}
    {parts.some((p) => p.error) && accepted && <Copy>{m.custos_total_parcial()}</Copy>}
    {parts.map((part) => part.warming ? <Copy key={part.id}>{m.custos_aquecendo_progresso({ maquina: part.label, lidos: part.warming.read, total: part.warming.total })}</Copy> : part.error ? <Copy key={part.id}>{`${part.label}: ${part.error}`}</Copy> : null)}
    {!selected && <Copy>{m.custos_sem_dados_periodo()}</Copy>}
    {!!selected && !loading && !accepted && <Copy>{m.home_usage_load_failed()}</Copy>}
  </View>;
}

export function useChartColors() {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  return [c.accent, theme.tokens.status.warning, theme.tokens.status.success, theme.tokens.status.error, c.accentText, c.muted, c.faint];
}

export function Composition({ label, items }: { label: string; items: { label: string; value: number; color: string; detail?: string }[] }) {
  const c = useSettingsColors();
  const total = items.reduce((sum, item) => sum + Math.max(0, item.value), 0);
  return <View accessible accessibilityRole="image" accessibilityLabel={`${label}: ${items.map((item) => `${item.label}: ${item.detail ?? number(item.value)} (${percent(item.value, total)})`).join(' · ')}`} style={{ flexDirection: 'row', height: 12, backgroundColor: c.hover, borderRadius: RADIUS.full, overflow: 'hidden' }}>
    {items.map((item, index) => <View key={index} style={{ width: `${total > 0 ? Math.max(0, item.value) / total * 100 : 0}%`, height: 12, backgroundColor: item.color }} />)}
  </View>;
}

export interface ChartSeries { key: string; label: string; color: string }
export interface ChartDay { key: string; values: (number | null)[]; detail?: string }

export function Columns({ label, days, series, format = number, axisFormat = compactNumber, onPress, lines = false, fit = false }: { label: string; days: ChartDay[]; series: ChartSeries[]; format?: (value: number) => string; axisFormat?: (value: number) => string; onPress?: (key: string) => void; lines?: boolean; fit?: boolean }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [selectedDay, setSelectedDay] = useState<string | null>(null);
  const [plotWidth, setPlotWidth] = useState(264);
  if (!days.length) return <Copy muted>{m.uso_sem_dias()}</Copy>;
  const describe = (day: ChartDay) => {
    const sum = day.values.some((value) => value === null) ? '—' : format(day.values.reduce<number>((value, part) => value + (part ?? 0), 0));
    return `${day.key}: ${sum}; ${day.values.map((value, index) => `${series[index].label}: ${value === null ? '—' : format(value)}`).join(' · ')}${day.detail ? ` · ${day.detail}` : ''}`;
  };
  const selected = days.find((day) => day.key === selectedDay);
  const maximum = Math.max(1, ...days.map((day) => lines ? Math.max(0, ...day.values.map((value) => value ?? 0)) : day.values.reduce<number>((sum, value) => sum + Math.max(0, value ?? 0), 0)));
  const step = fit ? plotWidth / days.length : Math.max(64, 264 / days.length);
  const width = days.length * step;
  const height = 180;
  return <View accessibilityLabel={label} style={{ gap: theme.base.space[2] }}><View style={{ flexDirection: 'row', gap: theme.base.space[2] }}>
    <View style={{ width: 64, flexShrink: 0, height, justifyContent: 'space-between' }}>{[maximum, maximum / 2, 0].map((value, index) => <Text key={index} numberOfLines={1} adjustsFontSizeToFit minimumFontScale={0.65} style={{ color: c.muted, fontSize: theme.base.text.sm }}>{axisFormat(value)}</Text>)}</View>
    <ScrollView horizontal scrollEnabled={!fit} onLayout={(event) => { if (fit) setPlotWidth(Math.max(1, event.nativeEvent.layout.width)); }} style={{ flex: 1, flexShrink: 1, minWidth: 0 }}><View style={{ width }}>
      <Svg width={width} height={height} accessible={false}>
        {[0, 0.25, 0.5, 0.75, 1].map((ratio) => <Line key={ratio} x1={0} x2={width} y1={height * ratio} y2={height * ratio} stroke={c.borderStrong} />)}
        {lines ? series.map((item, seriesIndex) => {
          const runs: string[][] = [[]];
          const points = days.flatMap((day, index) => {
            const value = day.values[seriesIndex];
            if (value === null) { if (runs.at(-1)!.length) runs.push([]); return []; }
            const x = index * step + step / 2;
            const y = height - 6 - Math.max(0, value) / maximum * (height - 12);
            runs.at(-1)!.push(`${x},${y}`);
            return [{ x, y, key: day.key }];
          });
          return <ReactFragment key={item.key}>{runs.filter((run) => run.length > 1).map((run, index) => <Polyline key={index} points={run.join(' ')} fill="none" stroke={item.color} strokeWidth={2} />)}{points.map((point) => <Circle key={point.key} cx={point.x} cy={point.y} r={3} fill={item.color} />)}</ReactFragment>;
        }) : days.map((day, index) => {
          let bottom = height;
          return day.values.map((value, segment) => {
            const size = Math.max(0, value ?? 0) / maximum * height;
            bottom -= size;
            return size > 0 ? <Rect key={`${day.key}:${segment}`} x={index * step + step * 0.22} y={bottom} width={step * 0.56} height={size} fill={series[segment].color} /> : null;
          });
        })}
      </Svg>
      {fit ? <Pressable accessibilityRole="button" accessibilityLabel={describe(selected ?? days[0])} onPress={(event) => {
        const day = days[Math.min(days.length - 1, Math.max(0, Math.floor(event.nativeEvent.locationX / step)))];
        if (day) { if (onPress) onPress(day.key); else setSelectedDay(day.key); }
      }} style={{ position: 'absolute', top: 0, left: 0, width, height, minHeight: 44 }} /> :
        <View style={{ position: 'absolute', top: 0, left: 0, flexDirection: 'row' }}>{days.map((day) => <Pressable key={day.key} accessibilityRole="button" accessibilityLabel={describe(day)} onPress={() => onPress ? onPress(day.key) : setSelectedDay(day.key)} style={{ width: step, height, minHeight: 44 }} />)}</View>}
      <View style={{ flexDirection: 'row', justifyContent: 'space-between' }}>{(fit ? [...new Set([days[0], days[Math.floor((days.length - 1) / 2)], days[days.length - 1]])] : days).map((day) => <Text key={day.key} numberOfLines={1} accessibilityLabel={day.key} style={{ ...(fit ? {} : { width: step }), paddingTop: theme.base.space[2], textAlign: 'center', color: c.muted, fontSize: theme.base.text.xxxs }}>{`${day.key.slice(8, 10)}/${day.key.slice(5, 7)}`}</Text>)}</View>
    </View></ScrollView>
  </View>{fit && <Choice label={m.custos_dia()} options={days.map((day) => ({ key: day.key, label: describe(day) }))} values={[]} max={1} closeOnSelect onChange={(keys) => { if (keys[0]) { if (onPress) onPress(keys[0]); else setSelectedDay(keys[0]); } }} />}{!onPress && selected && <Copy>{describe(selected)}</Copy>}</View>;
}

export function Bar({ label, value, max, detail, onPress, color, compact = false }: { label: string; value: number; max: number; detail: string; onPress?: () => void; color?: string; compact?: boolean }) {
  const c = useSettingsColors();
  const body = <View style={{ gap: SPACE[1.5], paddingVertical: SPACE[2] }}>{compact ? <View style={{ flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'space-between', gap: SPACE[2] }}><View style={{ flexShrink: 1 }}><Copy>{label}</Copy></View><Copy>{detail}</Copy></View> : <><Copy>{label}</Copy><Copy>{detail}</Copy></>}
    <View style={{ height: 5, backgroundColor: c.hover, borderRadius: RADIUS.xs }}><View style={{ height: 5, width: `${Math.max(0, Math.min(100, max > 0 ? value / max * 100 : 0))}%`, backgroundColor: color ?? c.accent, borderRadius: RADIUS.xs }} /></View>
  </View>;
  return onPress ? <Pressable accessibilityRole="button" accessibilityLabel={`${label}: ${detail}`} onPress={onPress} style={{ minHeight: 44 }}>{body}</Pressable> : <View accessible accessibilityLabel={`${label}: ${detail}`}>{body}</View>;
}
