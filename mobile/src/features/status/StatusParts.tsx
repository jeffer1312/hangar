import { memo, useState, type ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import Svg, { Circle, Line, Polygon, Polyline, Rect } from 'react-native-svg';
import { useUnistyles } from 'react-native-unistyles';
import { Icon, type IconName } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { GrowFill, Panel } from '../usage/CostsVisuals';
import { useSettingsColors, withAlpha } from '../config/colors';
import { hexParaRgb, type StatusPoint } from '@hangar/core';
import { INK, MONO, RADIUS, SPACE } from '../../theme/scale';

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

/** Índice tocado pela posição; toque sem posição (leitor de tela, teste) fica no último. */
function touched(locationX: number | undefined, toIndex: (x: number) => number, last: number): number {
  return typeof locationX === 'number' && Number.isFinite(locationX) ? Math.max(0, Math.min(last, toIndex(locationX))) : last;
}

/** Deslizar para cima ou para baixo no leitor de tela anda um ponto; começa no último. */
const STEP_ACTIONS = [{ name: 'increment' }, { name: 'decrement' }];
function stepped(action: string, current: number, last: number): number | null {
  if (action === 'increment') return Math.min(last, current + 1);
  if (action === 'decrement') return Math.max(0, current - 1);
  return null;
}

/**
 * Curva da última hora: área, linha da média e o ponto de agora; o toque mostra o minuto escolhido.
 * A escolha guarda o `ts` do ponto: o retrato seguinte desloca a série e o índice apontaria outro minuto.
 */
export const Sparkline = memo(function Sparkline({ points, color, format, label }: { points: StatusPoint[]; color: string; format: (v: number, ago: number) => string; label: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [width, setWidth] = useState(140);
  const [pickedTs, setPickedTs] = useState<string | null>(null);
  const height = 46;
  if (points.length < 2) return <View style={{ height }} />;
  const values = points.map((p) => p.v);
  const found = pickedTs === null ? -1 : points.findIndex((p) => p.ts === pickedTs);
  const picked = found >= 0 ? found : null;
  const min = Math.min(...values) * 0.92;
  const max = Math.max(...values) * 1.05 || 1;
  const x = (i: number) => i / (values.length - 1) * width;
  const y = (v: number) => height - (max === min ? 0.5 : (v - min) / (max - min)) * height;
  const line = values.map((v, i) => `${x(i)},${y(v)}`).join(' ');
  const avg = values.reduce((a, b) => a + b, 0) / values.length;
  const last = values.length - 1;
  const current = picked ?? last;
  const pick = (i: number | null) => setPickedTs(i === null ? null : points[i].ts);
  return <View style={{ gap: SPACE[1] }}>
    <Pressable accessibilityRole="adjustable" accessibilityLabel={label} hitSlop={SPACE[1]}
      accessibilityValue={{ text: format(values[current], last - current) }}
      accessibilityActions={STEP_ACTIONS}
      onAccessibilityAction={(e) => { const i = stepped(e.nativeEvent.actionName, current, last); if (i !== null) pick(i); }}
      onLayout={(e) => setWidth(Math.max(1, e.nativeEvent.layout.width))}
      onPress={(e) => {
        const i = touched(e.nativeEvent?.locationX, (px) => Math.round(px / width * last), last);
        haptic.selection();
        pick(picked === i ? null : i);
      }}
      style={{ height }}>
      <Svg width={width} height={height}>
        <Line x1={0} x2={width} y1={y(avg)} y2={y(avg)} stroke={c.borderStrong} strokeWidth={1} />
        <Polygon points={`0,${height} ${line} ${width},${height}`} fill={color} fillOpacity={0.12} />
        <Polyline points={line} fill="none" stroke={color} strokeWidth={2} strokeLinejoin="round" />
        {picked !== null ? <Line x1={x(picked)} x2={x(picked)} y1={0} y2={height} stroke={c.faint} strokeWidth={1} /> : null}
        <Circle cx={x(current)} cy={y(values[current])} r={4} fill={color} />
      </Svg>
    </Pressable>
    <Text numberOfLines={1} style={{ color: c.faint, fontSize: theme.base.text.xxxs }}>
      {picked !== null ? format(values[picked], last - picked) : ' '}
    </Text>
  </View>;
});

/** `key` identifica a hora entre retratos (o início dela); `hour` é o rótulo. */
export interface HourColumn { key: string; hour: string; slow: number; errors: number }

/** 24 colunas por hora: lentas embaixo, erros em cima; o toque mostra a hora. */
export const HourColumns = memo(function HourColumns({ hours, label, describe }: { hours: HourColumn[]; label: string; describe: (h: HourColumn) => string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [width, setWidth] = useState(300);
  const [pickedKey, setPickedKey] = useState<string | null>(null);
  const height = 70;
  const n = Math.max(1, hours.length);
  const max = Math.max(3, ...hours.map((h) => h.slow + h.errors));
  const step = width / n;
  const bar = Math.min(step * 0.6, 14);
  const ticks = hours.map((h, i) => ({ h, i })).filter(({ i }) => i % 6 === 0);
  const found = pickedKey === null ? -1 : hours.findIndex((h) => h.key === pickedKey);
  const picked = found >= 0 ? found : null;
  const last = n - 1;
  const current = picked ?? last;
  const pick = (i: number | null) => setPickedKey(i === null ? null : hours[i]?.key ?? null);
  return <View style={{ gap: SPACE[1] }}>
    <Pressable accessibilityRole="adjustable" accessibilityLabel={label}
      accessibilityValue={hours[current] ? { text: describe(hours[current]) } : undefined}
      accessibilityActions={STEP_ACTIONS}
      onAccessibilityAction={(e) => { const i = stepped(e.nativeEvent.actionName, current, last); if (i !== null) pick(i); }}
      onLayout={(e) => setWidth(Math.max(1, e.nativeEvent.layout.width))}
      onPress={(e) => {
        const i = touched(e.nativeEvent?.locationX, (px) => Math.floor(px / step), last);
        haptic.selection();
        pick(picked === i ? null : i);
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
});

/** Medidor de uso: nome, trilho com a parte usada e "usado / total". */
export function Meter({ name, used, total, text, label }: { name: string; used: number; total: number; text: string; label: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const ratio = total > 0 ? used / total : 0;
  const color = ratio >= 0.9 ? theme.tokens.status.error : ratio >= 0.75 ? theme.tokens.status.warning : c.accent;
  // Com fonte grande os dois textos crescem e quebram linha; o trilho fica com o que sobra, sem sumir.
  return <View accessible accessibilityLabel={label} style={{ minHeight: 32, flexDirection: 'row', alignItems: 'center', gap: SPACE[2] }}>
    <Text style={{ minWidth: 72, flexShrink: 1, color: c.muted, fontSize: theme.base.text.xs }}>{name}</Text>
    <View style={{ flex: 1, minWidth: 48, height: 8, borderRadius: RADIUS.full, overflow: 'hidden', backgroundColor: withAlpha(color, 0.18) }}>
      <GrowFill percent={ratio * 100} color={color} height={8} />
    </View>
    <Text style={{ minWidth: 92, flexShrink: 1, textAlign: 'right', color: c.text, fontSize: theme.base.text.xxs, fontVariant: ['tabular-nums'] }}>{text}</Text>
  </View>;
}

function rgbOf(color: string): [number, number, number] | null {
  const hex = hexParaRgb(color);
  if (hex) return hex;
  const m = /^rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(color);
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

/** Texto legível sobre a fatia: escuro em cor clara, branco em cor escura. */
function inkOn(color: string): string {
  const rgb = rgbOf(color);
  if (!rgb) return INK.onAccent;
  const [r, g, b] = rgb.map((v) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.4 ? DARK_INK : INK.onAccent;
}
const DARK_INK = 'rgba(0,0,0,0.85)';

/** Barra dividida alta: fatia com mais de 12% leva o próprio valor, alinhado à esquerda. */
export function SegBar({ label, items }: { label: string; items: { label: string; value: number; color: string; detail: string }[] }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const total = items.reduce((sum, it) => sum + Math.max(0, it.value), 0);
  return <View accessible accessibilityRole="image" accessibilityLabel={`${label}: ${items.map((it) => `${it.label} ${it.detail}`).join(' · ')}`}
    style={{ flexDirection: 'row', height: 24, borderRadius: RADIUS.sm, overflow: 'hidden', backgroundColor: c.hover }}>
    {items.map((it, i) => {
      const share = total > 0 ? Math.max(0, it.value) / total : 0;
      return <View key={i} style={{ width: `${share * 100}%`, height: 24, justifyContent: 'center', paddingHorizontal: SPACE[1.5], backgroundColor: it.color }}>
        {share > 0.12 ? <Text numberOfLines={1} style={{ color: inkOn(it.color), fontSize: theme.base.text.xxs, fontWeight: theme.base.weight.semibold }}>{it.detail}</Text> : null}
      </View>;
    })}
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
