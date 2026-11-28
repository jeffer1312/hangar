import { useEffect } from 'react';
import { Pressable, Text, View, type StyleProp, type ViewStyle } from 'react-native';
import Animated, { cancelAnimation, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as m from '../../paraglide/messages';

export type BalloonAction = { label: string; primary?: boolean; onPress: () => void };

/** Título e corpo de cada passo e dica do tour do celular. */
export function tourText(id: string): { title: string; body: string } {
  // Passo que virou dica ("tour-search") fala como o passo.
  switch (id.replace(/^tour-/, '')) {
    case 'new': return { title: m.native_tour_new_title(), body: m.mobile_tour_new_body() };
    case 'menu': return { title: m.mobile_tour_menu_title(), body: m.mobile_tour_menu_body() };
    case 'search': return { title: m.native_tour_search_title(), body: m.mobile_tour_search_body() };
    case 'attach': return { title: m.native_tour_attach_title(), body: m.mobile_tour_attach_body() };
    case 'dictation': return { title: m.native_tour_dictation_title(), body: m.mobile_tour_dictation_body() };
    case 'model': return { title: m.mobile_tour_model_title(), body: m.mobile_tour_model_body() };
    default: return { title: m.mobile_tour_tip_media_title(), body: m.mobile_tour_tip_media_body() };
  }
}

function WaitingDot() {
  const { theme } = useUnistyles();
  const reduced = useReducedMotion();
  const opacity = useSharedValue(1);
  useEffect(() => {
    if (reduced) return;
    opacity.value = withRepeat(withTiming(0.25, { duration: 600 }), -1, true);
    return () => cancelAnimation(opacity);
  }, [reduced, opacity]);
  const style = useAnimatedStyle(() => ({ opacity: opacity.value }));
  return <Animated.View style={[styles.dot, { backgroundColor: theme.tokens.accent.base }, style]} />;
}

/** O balão do tour e das dicas: o mesmo na tela real e na animação da novidade. */
export function TourBalloon({ tag, tip, title, body, waiting, actions, arrow, arrowX, style }: {
  tag: string; tip?: boolean; title: string; body: string; waiting?: boolean; actions: BalloonAction[];
  arrow?: 'up' | 'down' | null; arrowX?: number; style?: StyleProp<ViewStyle>;
}) {
  const { theme } = useUnistyles();
  const arrowStyle = arrow ? [styles.arrow, arrow === 'up' ? styles.arrowUp : styles.arrowDown,
    { left: arrowX ?? 24, backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.default }] : null;
  return <View style={[styles.balloon, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.default }, style]}
    accessibilityRole="alert" accessibilityLabel={`${title}. ${body}`}>
    {arrowStyle ? <View style={arrowStyle} /> : null}
    <View style={[styles.tag, { backgroundColor: tip ? theme.tokens.accent.dim : theme.tokens.fillSubtle }]}>
      <Text style={[styles.tagText, { color: tip ? theme.tokens.accent.base : theme.tokens.text.secondary }]}>{tag}</Text>
    </View>
    <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{title}</Text>
    <Text style={[styles.body, { color: theme.tokens.text.secondary }]}>{body}</Text>
    {waiting ? <View style={styles.waiting}>
      <WaitingDot />
      <Text style={[styles.waitingText, { color: theme.tokens.text.muted }]}>{m.native_tour_waiting()}</Text>
    </View> : null}
    <View style={styles.actions}>
      {actions.map((action) => <Pressable key={action.label} onPress={action.onPress} hitSlop={5} accessibilityRole="button"
        style={({ pressed }) => [styles.button, action.primary
          ? { backgroundColor: pressed ? theme.tokens.accent.press : theme.tokens.accent.base, borderColor: 'transparent' }
          : { borderColor: theme.tokens.border.default, backgroundColor: pressed ? theme.tokens.fillSubtle : 'transparent' }]}>
        <Text style={[styles.buttonText, { color: action.primary ? theme.tokens.text.inverse : theme.tokens.text.primary },
          action.primary && styles.buttonPrimary]}>{action.label}</Text>
      </Pressable>)}
    </View>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  balloon: { borderWidth: 1, borderRadius: theme.base.radius.lg, borderCurve: 'continuous', paddingHorizontal: theme.base.space[4],
    paddingVertical: theme.base.space[3], gap: theme.base.space[1], shadowColor: '#000', shadowOpacity: 0.35, shadowRadius: 18,
    shadowOffset: { width: 0, height: 8 }, elevation: 12 },
  arrow: { position: 'absolute', width: 14, height: 14, borderLeftWidth: 1, borderTopWidth: 1 },
  arrowUp: { top: -8, transform: [{ rotate: '45deg' }] },
  arrowDown: { bottom: -8, transform: [{ rotate: '225deg' }] },
  tag: { alignSelf: 'flex-start', borderRadius: theme.base.radius.full, paddingHorizontal: theme.base.space[2], paddingVertical: 1, marginBottom: theme.base.space[1] },
  tagText: { fontSize: theme.base.text.xxs, fontWeight: theme.base.weight.medium },
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  body: { fontSize: theme.base.text.xs, lineHeight: 18 },
  waiting: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], marginTop: theme.base.space[1] },
  dot: { width: 6, height: 6, borderRadius: theme.base.radius.full },
  waitingText: { fontSize: theme.base.text.xxs },
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2], marginTop: theme.base.space[2] },
  button: { minHeight: 34, justifyContent: 'center', paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md, borderWidth: 1 },
  buttonText: { fontSize: theme.base.text.xs },
  buttonPrimary: { fontWeight: theme.base.weight.semibold },
}));
