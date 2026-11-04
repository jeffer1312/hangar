import { useContext, useEffect, useMemo } from 'react';
import { View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import Animated, { cancelAnimation, Easing, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from 'react-native-reanimated';
import { sessionContextPct, type AggSession } from '@hangar/core';
import { ContextRing } from '../../chat/ContextRing';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { HangarMark } from '../../ui/HangarMark';
import { Icon } from '../../ui/Icon';
import { WorkingMarkPaused } from '../../ui/WorkingMark';

type AvatarSession = Pick<AggSession, 'provider' | 'engine' | 'limited' | 'status_line' | 'context'> & { state: AggSession['state'] | null };

// Abaixo disso o anel não se distingue da borda (filhos do grupo na aba).
const RING_MIN_SIZE = 22;

// `ctxPct`: quem já calculou (a linha da lista) passa; ausente, calcula aqui.
export function SessionAvatar({ session, size = 36, ctxPct: given }: { session: AvatarSession; size?: number; ctxPct?: number | null }) {
  const { theme } = useUnistyles();
  const { state, limited, provider, engine } = session;
  const ctxPct = useMemo(() => size < RING_MIN_SIZE ? null : given !== undefined ? given : sessionContextPct(session),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- só os campos lidos
    [size, given, session.status_line, session.context]);
  const working = state === 'working';
  const waiting = state === 'awaiting_input';
  const ready = state === 'idle';
  const warning = waiting || (ready && limited);
  const glyph = provider === 'claude' && engine && /gpt|openai|codex/i.test(engine) ? 'codex' : provider;
  const known = glyph && ['claude', 'codex', 'kimi', 'pi', 'omp'].includes(glyph);
  const accent = theme.tokens.accent.base;
  const reduced = useReducedMotion();
  const paused = useContext(WorkingMarkPaused);
  const pulse = useSharedValue(0);
  const turn = useSharedValue(0);
  const running = working && !reduced && !paused;
  useEffect(() => {
    if (!running) return;
    // A inversão do ciclo usa o valor de partida; retomar no pico apagaria a respiração.
    pulse.value = 0;
    pulse.value = withRepeat(withTiming(1, { duration: 800, easing: Easing.inOut(Easing.quad) }), -1, true);
    turn.value = withRepeat(withTiming(turn.value + 360, { duration: 1400, easing: Easing.linear }), -1, false);
    return () => {
      cancelAnimation(pulse);
      cancelAnimation(turn);
    };
  }, [running, pulse, turn]);
  const glow = useAnimatedStyle(() => ({
    opacity: reduced ? 0.2 : 0.12 + pulse.value * 0.2,
    transform: [{ scale: reduced ? 1 : 0.9 + pulse.value * 0.18 }],
  }));
  const orbit = useAnimatedStyle(() => ({ transform: [{ rotate: `${reduced ? 0 : turn.value}deg` }] }));
  return (
    <View testID="session-avatar" style={[styles.avatar, { width: size, height: size }]} pointerEvents="none"
      accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      {working ? <Animated.View style={[styles.glow, { borderRadius: size / 2, backgroundColor: accent, boxShadow: `0px 0px 10px ${accent}` }, glow]} /> : null}
      <View testID="session-avatar-ring" style={[styles.ring, { borderRadius: size / 2, borderColor: warning ? theme.tokens.status.warning : working && reduced ? accent : theme.tokens.border.default }]} />
      {/* O arco cobre a borda (inset 1, 1.5 de largura), começando no topo. */}
      {ctxPct !== null ? <View testID="session-avatar-context" style={styles.context}>
        <ContextRing pct={ctxPct} size={size} showValue={false} stroke={2} />
      </View> : null}
      {working ? <Animated.View style={[styles.orbit, orbit]}>
        <View testID="session-avatar-dot" style={[styles.dot, { left: (size - 5) / 2, backgroundColor: accent }]} />
      </Animated.View> : null}
      {known ? <ProviderGlyph provider={glyph} size={size * 20 / 36} inline color={glyph === 'codex' ? theme.tokens.text.primary : undefined} />
        : <HangarMark size={size * 20 / 36} color={theme.tokens.text.secondary} />}
      {ready ? <View testID="session-avatar-badge" style={[styles.badge, { backgroundColor: limited ? theme.tokens.status.warning : theme.tokens.status.success }]}>
        <Icon name={limited ? 'Hourglass' : 'Check'} size={10} color={theme.tokens.bg.base} />
      </View> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  avatar: { width: 36, height: 36, alignItems: 'center', justifyContent: 'center' },
  ring: { position: 'absolute', inset: 1, borderWidth: 1.5, borderRadius: theme.base.radius.full },
  glow: { position: 'absolute', inset: 3, borderRadius: theme.base.radius.full },
  orbit: { position: 'absolute', inset: 0 },
  context: { position: 'absolute', top: 0, left: 0 },
  dot: { position: 'absolute', top: -0.75, left: 15.5, width: 5, height: 5, borderRadius: theme.base.radius.full },
  badge: { position: 'absolute', right: -1, bottom: -1, width: 14, height: 14, borderRadius: theme.base.radius.full, alignItems: 'center', justifyContent: 'center' },
}));
