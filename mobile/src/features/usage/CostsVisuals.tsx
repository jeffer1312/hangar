import { useEffect, useState, type ReactNode } from 'react';
import { Pressable, ScrollView, Text, View, type TextStyle } from 'react-native';
import Animated, { Easing, FadeInDown, useAnimatedProps, useAnimatedStyle, useReducedMotion, useSharedValue, withDelay, withTiming } from 'react-native-reanimated';
import Svg, { Circle, Defs, Line, LinearGradient, Polygon, Polyline, Stop } from 'react-native-svg';
import { useUnistyles } from 'react-native-unistyles';
import { Icon, type IconName } from '../../ui/Icon';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { haptic } from '../../ui/haptics';
import { usePressScale } from '../../ui/pressScale';
import { useSettingsColors, withAlpha } from '../config/colors';
import { RADIUS, SPACE } from '../../theme/scale';

const EASE = Easing.out(Easing.cubic);
const AnimatedCircle = Animated.createAnimatedComponent(Circle);
const AnimatedPolyline = Animated.createAnimatedComponent(Polyline);
const AnimatedLine = Animated.createAnimatedComponent(Line);

// Marca do agente quando existe logo; o resto fica com o terminal.
export function sourceBrand(key: string): string | null {
  const brand = key.startsWith('anthropic:') ? 'claude' : key.startsWith('codex:') ? 'codex' : key;
  return ['claude', 'codex', 'kimi'].includes(brand) ? brand : null;
}

export function SourceMark({ source, size = 15, color }: { source: string; size?: number; color: string }) {
  const brand = sourceBrand(source);
  return brand ? <ProviderGlyph provider={brand} size={size} inline /> : <Icon name="Terminal" size={size} color={color} />;
}

export function Segmented({ label, options, value, onChange }: { label: string; options: { key: string; label: string; disabled?: boolean }[]; value: string; onChange: (key: string) => void }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const reduced = useReducedMotion();
  const [frames, setFrames] = useState<Record<string, { x: number; width: number }>>({});
  const x = useSharedValue(0);
  const width = useSharedValue(0);
  const frame = frames[value];
  useEffect(() => {
    if (!frame) return;
    const config = { duration: reduced || width.value === 0 ? 0 : 240, easing: EASE };
    x.value = withTiming(frame.x, config);
    width.value = withTiming(frame.width, config);
  }, [frame?.x, frame?.width, reduced]);
  const pill = useAnimatedStyle(() => ({ width: width.value, transform: [{ translateX: x.value }] }));
  return <View accessibilityRole="tablist" accessibilityLabel={label} style={{ flexDirection: 'row', alignSelf: 'flex-start', borderRadius: RADIUS.full, backgroundColor: c.inset, borderWidth: 1, borderColor: c.border }}>
    <Animated.View style={[{ position: 'absolute', top: 4, bottom: 4, left: 0, borderRadius: RADIUS.full, backgroundColor: c.accentDim }, pill]} />
    {options.map((option) => <Pressable key={option.key} accessibilityRole="tab" accessibilityLabel={option.label} accessibilityState={{ selected: option.key === value, disabled: option.disabled }} disabled={option.disabled}
      onLayout={(event) => { const { x: left, width: size } = event.nativeEvent.layout; setFrames((all) => all[option.key]?.x === left && all[option.key]?.width === size ? all : { ...all, [option.key]: { x: left, width: size } }); }}
      onPress={() => { if (option.key !== value) { haptic.selection(); onChange(option.key); } }}
      style={{ minHeight: 44, minWidth: 44, paddingHorizontal: SPACE[3], alignItems: 'center', justifyContent: 'center', opacity: option.disabled ? 0.45 : 1 }}>
      <Text style={{ color: option.key === value ? c.text : c.muted, fontSize: theme.base.text.xs, fontWeight: option.key === value ? theme.base.weight.semibold : theme.base.weight.normal }}>{option.label}</Text>
    </Pressable>)}
  </View>;
}

export function Notice({ title, details = [] }: { title: string; details?: string[] }) {
  const { theme } = useUnistyles();
  const warning = theme.tokens.status.warning;
  return <View accessibilityRole="alert" style={{ flexDirection: 'row', gap: SPACE[2], alignItems: 'flex-start', padding: SPACE[3], borderRadius: theme.base.radius.md, backgroundColor: withAlpha(warning, 0.12) }}>
    <Icon name="TriangleAlert" size={16} color={warning} />
    <View style={{ flex: 1, gap: SPACE[1] }}>
      <Text style={{ color: warning, fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold }}>{title}</Text>
      {details.map((detail) => <Text key={detail} style={{ color: warning, fontSize: theme.base.text.xxs, opacity: 0.85 }}>{detail}</Text>)}
    </View>
  </View>;
}

export function SourceChips({ items }: { items: { key: string; label: string; detail: string; selected: boolean; all?: boolean; onPress: () => void }[] }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={{ gap: SPACE[2], paddingRight: SPACE[6] }}>
    {items.map((item) => <Pressable key={item.key} accessibilityRole="button" accessibilityLabel={item.all ? item.label : `${item.label} · ${item.detail}`} accessibilityState={{ selected: item.selected }}
      onPress={() => { haptic.selection(); item.onPress(); }}
      style={{ minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: SPACE[1.5], paddingHorizontal: SPACE[3], borderRadius: RADIUS.full, borderWidth: 1, borderColor: item.selected ? withAlpha(c.accent, 0.5) : c.border, backgroundColor: item.selected ? c.accentDim : c.inset }}>
      {item.all ? <Icon name="Layers" size={15} color={c.muted} /> : <SourceMark source={item.key} color={c.muted} />}
      <Text style={{ color: item.selected ? c.text : c.muted, fontSize: theme.base.text.xs }}>{item.label}</Text>
      <Text style={{ color: c.text, fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, fontVariant: ['tabular-nums'] }}>{item.detail}</Text>
    </Pressable>)}
  </ScrollView>;
}

// O valor certo desde o primeiro quadro; cada troca entra subindo, na thread de UI.
export function RiseNumber({ text, style }: { text: string; style: TextStyle }) {
  const reduced = useReducedMotion();
  return <Animated.Text key={text} entering={reduced ? undefined : FadeInDown.duration(420)} style={style}>{text}</Animated.Text>;
}

export function GrowFill({ percent, color, height = 5, delay = 0 }: { percent: number; color: string; height?: number; delay?: number }) {
  const reduced = useReducedMotion();
  const grow = useSharedValue(reduced ? 1 : 0);
  useEffect(() => { grow.value = reduced ? 1 : withDelay(delay, withTiming(1, { duration: 700, easing: EASE })); }, [reduced]);
  const style = useAnimatedStyle(() => ({ transform: [{ scaleX: grow.value }] }));
  return <Animated.View style={[{ height, width: `${Math.max(0, Math.min(100, percent))}%`, backgroundColor: color, borderRadius: RADIUS.xs, transformOrigin: 'left' }, style]} />;
}

// O título fica filho direto do cartão (o ícone flutua ao lado) para quem lê o cartão pelo título achar o conteúdo.
export function Panel({ title, icon, children }: { title: string; icon?: IconName; children: ReactNode }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View style={{ padding: SPACE[4], gap: SPACE[3], backgroundColor: c.inset, borderRadius: theme.base.radius.lg }}>
    {icon && <View style={{ position: 'absolute', top: SPACE[4] + 3, left: SPACE[4] }}><Icon name={icon} size={16} color={c.accent} /></View>}
    <Text accessibilityRole="header" style={{ color: c.text, fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold, paddingLeft: icon ? SPACE[6] : 0 }}>{title}</Text>
    {children}
  </View>;
}

export function KpiTile({ icon, color, label, value, caption, exact, index }: { icon: IconName; color: string; label: string; value: string; caption: string; exact?: ReactNode; index: number }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const reduced = useReducedMotion();
  const press = usePressScale();
  const [open, setOpen] = useState(false);
  return <Animated.View entering={reduced ? undefined : FadeInDown.delay(80 + index * 50).duration(360)} style={[{ flexGrow: 1, flexBasis: '45%', minWidth: 140 }, press.style]}>
    <Pressable accessibilityRole="button" accessibilityLabel={`${label}: ${value}. ${caption}`} accessibilityState={{ expanded: open }}
      onPressIn={press.onPressIn} onPressOut={press.onPressOut} onPress={() => { haptic.selection(); setOpen(!open); }}
      style={{ minHeight: 44, padding: SPACE[3], gap: SPACE[1], borderRadius: theme.base.radius.lg, backgroundColor: c.inset, borderWidth: 1, borderColor: open ? withAlpha(c.accent, 0.45) : 'transparent' }}>
      <View style={{ width: 28, height: 28, borderRadius: RADIUS.sm, alignItems: 'center', justifyContent: 'center', backgroundColor: withAlpha(color, 0.16), marginBottom: SPACE[1] }}><Icon name={icon} size={15} color={color} /></View>
      <Text style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{label}</Text>
      <Text numberOfLines={1} adjustsFontSizeToFit minimumFontScale={0.7} style={{ color: c.text, fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold, fontVariant: ['tabular-nums'] }}>{value}</Text>
      <Text numberOfLines={open ? undefined : 1} style={{ color: c.muted, fontSize: theme.base.text.xxs }}>{caption}</Text>
      {open && exact}
    </Pressable>
  </Animated.View>;
}

export function Ring({ fraction, color, value, label, size = 92 }: { fraction: number; color: string; value: string; label: string; size?: number }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const reduced = useReducedMotion();
  const radius = 40;
  const length = 2 * Math.PI * radius;
  const target = Math.max(0, Math.min(1, fraction));
  const fill = useSharedValue(reduced ? target : 0);
  useEffect(() => { fill.value = reduced ? target : withTiming(target, { duration: 1100, easing: EASE }); }, [target, reduced]);
  const props = useAnimatedProps(() => ({ strokeDashoffset: length * (1 - fill.value) }));
  return <View accessible accessibilityRole="image" accessibilityLabel={`${label}: ${value}`} style={{ width: size, height: size }}>
    <Svg width={size} height={size} viewBox="0 0 100 100">
      <Circle cx={50} cy={50} r={radius} stroke={c.hover} strokeWidth={10} fill="none" />
      <AnimatedCircle cx={50} cy={50} r={radius} stroke={color} strokeWidth={10} fill="none" strokeLinecap="round" strokeDasharray={`${length} ${length}`} animatedProps={props} transform="rotate(-90 50 50)" />
    </Svg>
    <View style={{ position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, alignItems: 'center', justifyContent: 'center' }}>
      <Text style={{ color: c.text, fontSize: theme.base.text.base, fontWeight: theme.base.weight.bold, fontVariant: ['tabular-nums'] }}>{value}</Text>
      <Text style={{ color: c.muted, fontSize: theme.base.text.xxxs }}>{label}</Text>
    </View>
  </View>;
}

// Gasto acumulado do mês: linha que se desenha, projeção tracejada até o último dia e o mês passado pontilhado.
export function ForecastLine({ cumulative, daysInMonth, projected, lastMonth, label }: { cumulative: number[]; daysInMonth: number; projected: number; lastMonth: number | null; label: string }) {
  const c = useSettingsColors();
  const reduced = useReducedMotion();
  const [width, setWidth] = useState(280);
  const height = 96;
  const top = Math.max(projected, lastMonth ?? 0, cumulative.at(-1) ?? 0, 1) * 1.05;
  const x = (index: number) => index / Math.max(1, daysInMonth - 1) * width;
  const y = (value: number) => height - 4 - value / top * (height - 8);
  const points = cumulative.map((value, index) => ({ x: x(index), y: y(value) }));
  const length = points.reduce((sum, point, index) => index ? sum + Math.hypot(point.x - points[index - 1].x, point.y - points[index - 1].y) : 0, 0) || 1;
  const end = points.at(-1) ?? { x: 0, y: height };
  const draw = useSharedValue(reduced ? 1 : 0);
  useEffect(() => { draw.value = reduced ? 1 : withTiming(1, { duration: 1100, easing: EASE }); }, [reduced, width, cumulative.length]);
  const lineProps = useAnimatedProps(() => ({ strokeDashoffset: length * (1 - draw.value) }));
  const projectionProps = useAnimatedProps(() => ({ opacity: Math.max(0, (draw.value - 0.8) * 5) }));
  return <View accessible accessibilityRole="image" accessibilityLabel={label} onLayout={(event) => setWidth(Math.max(1, event.nativeEvent.layout.width))} style={{ height }}>
    <Svg width={width} height={height}>
      <Defs><LinearGradient id="forecastArea" x1="0" x2="0" y1="0" y2="1"><Stop offset="0" stopColor={c.accent} stopOpacity={0.3} /><Stop offset="1" stopColor={c.accent} stopOpacity={0} /></LinearGradient></Defs>
      {lastMonth !== null && <Line x1={0} x2={width} y1={y(lastMonth)} y2={y(lastMonth)} stroke={c.muted} strokeDasharray="2 4" strokeOpacity={0.7} />}
      {points.length > 1 && <Polygon points={`0,${height} ${points.map((p) => `${p.x},${p.y}`).join(' ')} ${end.x},${height}`} fill="url(#forecastArea)" />}
      <AnimatedPolyline points={points.map((p) => `${p.x},${p.y}`).join(' ')} fill="none" stroke={c.accent} strokeWidth={2.2} strokeDasharray={`${length} ${length}`} animatedProps={lineProps} />
      <AnimatedLine x1={end.x} y1={end.y} x2={width - 4} y2={y(projected)} stroke={c.accent} strokeWidth={2} strokeDasharray="4 4" animatedProps={projectionProps} />
      <AnimatedCircle cx={width - 4} cy={y(projected)} r={3.5} fill={c.accent} animatedProps={projectionProps} />
    </Svg>
  </View>;
}
