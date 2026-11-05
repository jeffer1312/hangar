import { forwardRef, useImperativeHandle, useRef, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import Animated, { FadeIn } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { AnchoredPanel } from '../../ui/AnchoredPanel';
import { SettingsList } from '../../ui/SettingsList';
import * as m from '../../paraglide/messages';
import { Icon, type IconName } from '../../ui/Icon';
import { usePressScale } from '../../ui/pressScale';

export type PillAction = { id: string; title: string; subtitle?: string; state?: 'on' | 'off'; attributes?: { disabled?: boolean } };

const AnimatedPressable = Animated.createAnimatedComponent(Pressable);

type Props = {
  icon?: IconName;
  label: string;
  // Nome da escolha para o leitor de tela: "Máquina: Notebook".
  aria: string;
  disabled?: boolean;
  actions?: PillAction[];
  menuTitle?: string;
  onAction?: (id: string) => void;
  onPress?: () => void;
  // Estado que a pessoa precisa ver sem abrir o menu (conta sem cota, troca automática).
  tone?: 'warning' | 'accent';
  // Ação direta: sem a seta, porque não abre menu.
  plain?: boolean;
};

// A pílula quieta do app de PC (quiet_pill): 26 de altura, texto 12.5 apagado, ícone 14 e a seta do
// menu. O hitSlop leva o toque aos 44 pt sem engordar o desenho.
export const QuietPill = forwardRef<View, Props>(function QuietPill(
  { icon, label, aria, disabled, actions, menuTitle, onAction, onPress, tone, plain }, ref,
) {
  const anchor = useRef<View>(null);
  useImperativeHandle(ref, () => anchor.current!, []);
  const [open, setOpen] = useState(false);
  const { theme } = useUnistyles();
  const ink = tone === 'warning' ? theme.tokens.status.warning : tone === 'accent' ? theme.tokens.accent.base : theme.tokens.text.muted;
  const press = usePressScale();
  const body = (
    <AnimatedPressable
      ref={anchor}
      onPress={actions ? () => setOpen(true) : onPress}
      onPressIn={press.onPressIn}
      onPressOut={press.onPressOut}
      disabled={disabled}
      hitSlop={{ top: 9, bottom: 9, left: 2, right: 2 }}
      accessibilityRole="button"
      accessibilityLabel={`${aria}: ${label}`}
      accessibilityState={{ disabled: !!disabled }}
      style={[styles.pill, press.style, disabled && styles.disabled]}
    >
      {icon ? <Icon name={icon} size={14} color={ink} /> : null}
      <Text style={[styles.text, tone && { color: ink }]} numberOfLines={1}>{label}</Text>
      {plain ? null : <Icon name="ChevronDown" size={12} color={theme.tokens.text.muted} />}
    </AnimatedPressable>
  );
  return (
    <Animated.View entering={FadeIn.duration(220)} style={styles.shrink}>
      {body}
      {actions ? <AnchoredPanel open={open && !disabled} anchor={anchor.current} onClose={() => setOpen(false)}
        label={menuTitle ?? aria} closeLabel={m.native_close()} radius={12} padding={4}>
        <SettingsList items={actions.map((it) => ({ id: it.id, label: it.title, hint: it.subtitle,
          selected: it.state === 'on', disabled: it.attributes?.disabled }))}
          onPick={(id) => { setOpen(false); onAction?.(id); }} />
      </AnchoredPanel> : null}
    </Animated.View>
  );
});

const styles = StyleSheet.create((theme) => ({
  shrink: { flexShrink: 1, minWidth: 0 },
  pill: {
    height: 26,
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.sm,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1.5],
  },
  text: { flexShrink: 1, maxWidth: 220, fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  disabled: { opacity: 0.5 },
}));
