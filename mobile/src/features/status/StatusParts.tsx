import { useState, type ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import Svg, { Circle, Line, Polygon, Polyline, Rect } from 'react-native-svg';
import { useUnistyles } from 'react-native-unistyles';
import { Icon, type IconName } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { GrowFill, Panel } from '../usage/CostsVisuals';
import { useSettingsColors, withAlpha } from '../config/colors';
import { MONO, RADIUS, SPACE } from '../../theme/scale';

/** Cartão de um bloco: título, a linha de resumo embaixo dele (como no celular do mock) e o realce de quem veio da lista de atenção. */
export function Block({ title, hint, right, flash, onLayout, children }: {
  title: string; hint?: string; right?: ReactNode; flash?: boolean; onLayout?: (y: number) => void; children: ReactNode;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View onLayout={(e) => onLayout?.(e.nativeEvent.layout.y)}
    style={{ borderRadius: RADIUS.lg, borderWidth: 1, borderColor: flash ? withAlpha(c.accent, 0.7) : 'transparent' }}>
    <Panel title={title}>
      {hint ? <Text style={{ color: c.muted, fontSize: theme.base.text.xs, marginTop: -SPACE[2] }}>{hint}</Text> : null}
      {right}
      {children}
    </Panel>
  </View>;
}

export function Chip({ text, tone, icon }: { text: string; tone?: 'bad' | 'ok'; icon?: IconName }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const color = tone === 'bad' ? theme.tokens.status.error : c.muted;
  return <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[1], paddingHorizontal: SPACE[2], paddingVertical: SPACE[1],
    borderRadius: RADIUS.full, borderWidth: 1, borderColor: tone === 'bad' ? withAlpha(color, 0.4) : c.border,
    backgroundColor: tone === 'bad' ? withAlpha(color, 0.1) : 'transparent' }}>
    {icon ? <Icon name={icon} size={13} color={tone === 'ok' ? theme.tokens.status.success : color} /> : null}
    <Text style={{ color, fontSize: theme.base.text.xxs }}>{text}</Text>
  </View>;
}

export function Chips({ children }: { children: ReactNode }) {
  return <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: SPACE[1.5] }}>{children}</View>;
}

export function Legend({ items }: { items: { label: string; color: string; extra?: string; shape?: 'dot' | 'diamond' }[] }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View style={{ flexDirection: 'row', flexWrap: 'wrap', columnGap: SPACE[3], rowGap: SPACE[1] }}>
    {items.map((item) => <View key={item.label} style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[1] }}>
      <View style={{ width: 8, height: 8, backgroundColor: item.color, borderRadius: item.shape === 'dot' ? RADIUS.full : 2,
        transform: item.shape === 'diamond' ? [{ rotate: '45deg' }] : undefined }} />
      <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{item.label}</Text>
      {item.extra ? <Text style={{ color: c.faint, fontSize: theme.base.text.xxs }}>{item.extra}</Text> : null}
    </View>)}
  </View>;
}

/** Curva da última hora: área, linha da média e o ponto de agora; o toque mostra o minuto escolhido. */
export function Sparkline({ values, color, format, label }: { values: number[]; color: string; format: (v: number, ago: number) => string; label: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [width, setWidth] = useState(140);
  const [picked, setPicked] = useState<number | null>(null);
  const height = 46;
  if (values.length < 2) return <View style={{ height }} />;
  const min = Math.min(...values) * 0.92;
  const max = Math.max(...values) * 1.05 || 1;
  const x = (i: number) => i / (values.length - 1) * width;
  const y = (v: number) => height - (max === min ? 0.5 : (v - min) / (max - min)) * height;
  const line = values.map((v, i) => `${x(i)},${y(v)}`).join(' ');
  const avg = values.reduce((a, b) => a + b, 0) / values.length;
  const last = values.length - 1;
  return <View style={{ gap: SPACE[1] }}>
    <Pressable accessibilityRole="image" accessibilityLabel={label} hitSlop={SPACE[1]}
      onLayout={(e) => setWidth(Math.max(1, e.nativeEvent.layout.width))}
      onPress={(e) => {
        const i = Math.max(0, Math.min(last, Math.round(e.nativeEvent.locationX / width * last)));
        haptic.selection();
        setPicked(picked === i ? null : i);
      }}
      style={{ height }}>
      <Svg width={width} height={height}>
        <Line x1={0} x2={width} y1={y(avg)} y2={y(avg)} stroke={c.borderStrong} strokeWidth={1} />
        <Polygon points={`0,${height} ${line} ${width},${height}`} fill={color} fillOpacity={0.12} />
        <Polyline points={line} fill="none" stroke={color} strokeWidth={2} strokeLinejoin="round" />
        {picked !== null ? <Line x1={x(picked)} x2={x(picked)} y1={0} y2={height} stroke={c.faint} strokeWidth={1} /> : null}
        <Circle cx={x(picked ?? last)} cy={y(values[picked ?? last])} r={4} fill={color} />
      </Svg>
    </Pressable>
    <Text numberOfLines={1} style={{ color: c.faint, fontSize: theme.base.text.xxxs }}>
      {picked !== null ? format(values[picked], last - picked) : ' '}
    </Text>
  </View>;
}

export interface HourColumn { hour: string; slow: number; errors: number }

/** 24 colunas por hora: lentas embaixo, erros em cima; o toque mostra a hora. */
export function HourColumns({ hours, label, describe }: { hours: HourColumn[]; label: string; describe: (h: HourColumn) => string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [width, setWidth] = useState(300);
  const [picked, setPicked] = useState<number | null>(null);
  const height = 70;
  const n = Math.max(1, hours.length);
  const max = Math.max(3, ...hours.map((h) => h.slow + h.errors));
  const step = width / n;
  const bar = Math.min(step * 0.6, 14);
  const ticks = hours.map((h, i) => ({ h, i })).filter(({ i }) => i % 6 === 0);
  return <View style={{ gap: SPACE[1] }}>
    <Pressable accessibilityRole="image" accessibilityLabel={label}
      onLayout={(e) => setWidth(Math.max(1, e.nativeEvent.layout.width))}
      onPress={(e) => {
        const i = Math.max(0, Math.min(n - 1, Math.floor(e.nativeEvent.locationX / step)));
        haptic.selection();
        setPicked(picked === i ? null : i);
      }}
      style={{ height }}>
      <Svg width={width} height={height}>
        {[0.5, 0].map((f) => <Line key={f} x1={0} x2={width} y1={height * f} y2={height * f} stroke={c.border} strokeWidth={1} />)}
        <Line x1={0} x2={width} y1={height} y2={height} stroke={c.borderStrong} strokeWidth={1} />
        {picked !== null ? <Rect x={picked * step} y={0} width={step} height={height} fill={c.hover} /> : null}
        {hours.map((h, i) => {
          const hs = h.slow / max * height;
          const he = h.errors / max * height;
          const left = i * step + (step - bar) / 2;
          return [
            hs > 0 ? <Rect key={`s${i}`} x={left} y={height - hs} width={bar} height={hs} rx={3} fill={theme.tokens.status.warning} /> : null,
            he > 0 ? <Rect key={`e${i}`} x={left} y={height - hs - he - (hs > 0 ? 2 : 0)} width={bar} height={he} rx={3} fill={theme.tokens.status.error} /> : null,
          ];
        })}
      </Svg>
    </Pressable>
    <View style={{ height: theme.base.text.xxxs + SPACE[1] }}>
      {ticks.map(({ h, i }) => <Text key={i} style={{ position: 'absolute', left: i * step, color: c.faint, fontSize: theme.base.text.xxxs }}>{h.hour}</Text>)}
    </View>
    {picked !== null && hours[picked] ? <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{describe(hours[picked])}</Text> : null}
  </View>;
}

/** Medidor de uso: nome, trilho com a parte usada e "usado / total". */
export function Meter({ name, used, total, text, label }: { name: string; used: number; total: number; text: string; label: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const ratio = total > 0 ? used / total : 0;
  const color = ratio >= 0.9 ? theme.tokens.status.error : ratio >= 0.75 ? theme.tokens.status.warning : c.accent;
  return <View accessible accessibilityLabel={label} style={{ minHeight: 32, flexDirection: 'row', alignItems: 'center', gap: SPACE[2] }}>
    <Text numberOfLines={1} style={{ width: 72, color: c.muted, fontSize: theme.base.text.xs }}>{name}</Text>
    <View style={{ flex: 1, height: 8, borderRadius: RADIUS.full, overflow: 'hidden', backgroundColor: withAlpha(color, 0.18) }}>
      <GrowFill percent={ratio * 100} color={color} height={8} />
    </View>
    <Text numberOfLines={1} style={{ minWidth: 92, textAlign: 'right', color: c.text, fontSize: theme.base.text.xxs, fontVariant: ['tabular-nums'] }}>{text}</Text>
  </View>;
}

/** Três barrinhas de sinal pela latência. */
export function Signal({ ms }: { ms: number }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const on = ms < 60 ? 3 : ms < 150 ? 2 : 1;
  return <View style={{ flexDirection: 'row', alignItems: 'flex-end', gap: 1 }}>
    {[5, 8, 11].map((h, i) => <View key={h} style={{ width: 3, height: h, borderRadius: 1, backgroundColor: i < on ? theme.tokens.status.success : c.borderStrong }} />)}
  </View>;
}

export function Mono({ children, color }: { children: ReactNode; color?: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <Text style={{ fontFamily: MONO, color: color ?? c.text, fontSize: theme.base.text.xxs }}>{children}</Text>;
}

/** Botão de ação dos cartões de erro e parado: 44 pt, ícone e texto. */
export function ActionButton({ label, icon, onPress }: { label: string; icon: IconName; onPress: () => void }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <Pressable accessibilityRole="button" accessibilityLabel={label} onPress={() => { haptic.selection(); onPress(); }}
    style={({ pressed }) => ({ minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: SPACE[1.5], paddingHorizontal: SPACE[3],
      borderRadius: RADIUS.sm, borderWidth: 1, borderColor: c.borderStrong, backgroundColor: pressed ? c.hover : 'transparent' })}>
    <Icon name={icon} size={15} color={c.text} />
    <Text style={{ color: c.text, fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium }}>{label}</Text>
  </Pressable>;
}
