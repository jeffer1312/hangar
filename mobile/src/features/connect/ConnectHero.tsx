import { useEffect } from 'react';
import { Pressable, Text, View } from 'react-native';
import Animated, { Easing, useAnimatedProps, useReducedMotion, useSharedValue, withDelay, withTiming, type SharedValue } from 'react-native-reanimated';
import Svg, { Path } from 'react-native-svg';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from '../../ui/Icon';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

const AnimatedPath = Animated.createAnimatedComponent(Path);
const ease = Easing.out(Easing.cubic);

function Arc({ d, length, progress }: { d: string; length: number; progress: SharedValue<number> }) {
  // Lacuna maior que o traço: em repouso nem a ponta redonda aparece.
  const props = useAnimatedProps(() => ({ strokeDashoffset: (length + 1) * (1 - progress.value) }));
  return <AnimatedPath d={d} strokeDasharray={[length, length + 2]} animatedProps={props} />;
}

// Os três arcos da marca da página (`assets/brand/mark.svg`), com o comprimento de cada um (raio × 240°, 216°, 192°).
const CONNECT_MARK_ARCS = [
  { d: 'M 34.18 154.00 A 76.00 76.00 0 1 1 165.82 154.00', length: 319 },
  { d: 'M 49.59 132.38 A 53.00 53.00 0 1 1 150.41 132.38', length: 200 },
  { d: 'M 70.16 119.14 A 30.00 30.00 0 1 1 129.84 119.14', length: 101 },
] as const;

/** A marca do Hangar se desenhando de fora para dentro, uma vez, ao abrir. */
export function ConnectMark({ size = 72 }: { size?: number }) {
  const { theme } = useUnistyles();
  const reduced = useReducedMotion();
  const outer = useSharedValue(reduced ? 1 : 0);
  const middle = useSharedValue(reduced ? 1 : 0);
  const inner = useSharedValue(reduced ? 1 : 0);
  useEffect(() => {
    if (reduced) return;
    outer.value = withTiming(1, { duration: 520, easing: ease });
    middle.value = withDelay(160, withTiming(1, { duration: 480, easing: ease }));
    inner.value = withDelay(320, withTiming(1, { duration: 440, easing: ease }));
  }, [reduced, outer, middle, inner]);
  const [a, b, c] = CONNECT_MARK_ARCS;
  return <View accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <Svg width={size} height={size} viewBox="0 0 200 200" fill="none" stroke={theme.tokens.accent.base} strokeWidth={13} strokeLinecap="round">
      <Arc d={a.d} length={a.length} progress={outer} />
      <Arc d={b.d} length={b.length} progress={middle} />
      <Arc d={c.d} length={c.length} progress={inner} />
    </Svg>
  </View>;
}

/** Quem ainda não tem conta: o que o Connect dá e os dois caminhos. */
export function ConnectHero({ notice, onCreate, onLogin }: { notice?: string; onCreate: () => void; onLogin: () => void }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const perks = [m.connect_perk_networks(), m.connect_perk_approve(), m.connect_perk_one_account()];
  return <View style={[styles.card, { borderColor: c.border }]}>
    <ConnectMark />
    <Text accessibilityRole="header" style={[styles.title, { color: c.text }]}>{m.connect_title()}</Text>
    <Text style={[styles.subtitle, { color: c.muted }]}>{m.connect_hero_subtitle()}</Text>
    <View style={[styles.badge, { backgroundColor: c.accentDim }]}><Text style={[styles.badgeText, { color: c.accentText }]}>{m.connect_free()}</Text></View>
    {notice ? <Text accessibilityRole="alert" style={[styles.notice, { color: theme.tokens.status.error }]}>{notice}</Text> : null}
    <View style={styles.perks}>
      {perks.map((perk) => <View key={perk} style={styles.perk}>
        <Icon name="Check" size={16} color={theme.tokens.status.success} />
        <Text style={[styles.perkText, { color: c.text }]}>{perk}</Text>
      </View>)}
    </View>
    <Pressable accessibilityRole="button" onPress={onCreate} style={[styles.primary, { backgroundColor: theme.tokens.accent.base }]}>
      <Text style={[styles.primaryText, { color: theme.ink.onAccent }]}>{m.connect_create_free()}</Text>
    </Pressable>
    <Pressable accessibilityRole="button" onPress={onLogin} style={[styles.secondary, { borderColor: c.borderStrong }]}>
      <Text style={[styles.secondaryText, { color: c.text }]}>{m.connect_have_account()}</Text>
    </Pressable>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  card: { alignItems: 'center', gap: theme.base.space[3], padding: theme.base.space[5], borderRadius: theme.base.radius.lg, borderWidth: 1 },
  title: { fontSize: theme.base.text.xl, fontWeight: theme.base.weight.bold },
  subtitle: { fontSize: theme.base.text.base, textAlign: 'center', lineHeight: 22 },
  badge: { borderRadius: theme.base.radius.full, paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[1] },
  badgeText: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold },
  notice: { fontSize: theme.base.text.sm, textAlign: 'center' },
  perks: { alignSelf: 'stretch', gap: theme.base.space[2], paddingVertical: theme.base.space[1] },
  perk: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[2] },
  perkText: { flex: 1, fontSize: theme.base.text.sm, lineHeight: 20 },
  primary: { alignSelf: 'stretch', minHeight: 52, borderRadius: theme.base.radius.md, justifyContent: 'center', alignItems: 'center' },
  primaryText: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  secondary: { alignSelf: 'stretch', minHeight: 48, borderRadius: theme.base.radius.md, borderWidth: 1, justifyContent: 'center', alignItems: 'center' },
  secondaryText: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.medium },
}));
