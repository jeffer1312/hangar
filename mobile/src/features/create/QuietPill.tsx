import { forwardRef, useImperativeHandle, useRef, useState, type ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import Animated, { FadeIn } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { AnchoredPanel } from '../../ui/AnchoredPanel';
import { SettingsList } from '../../ui/SettingsList';
import * as m from '../../paraglide/messages';
import { Icon, type IconName } from '../../ui/Icon';
import { usePressScale } from '../../ui/pressScale';

export type PillAction = { id: string; title: string; subtitle?: string; state?: 'on' | 'off'; attributes?: { disabled?: boolean };
  // Etiqueta da linha (ícone e cor), como na lista de sessões.
  leading?: ReactNode; titleColor?: string };

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
  // Ícone próprio no lugar de `icon` (etiqueta da conta, com a cor dela).
  leading?: ReactNode;
  // Segunda parte do texto ("worktree nova"): visível após " · ", lida como dica, fora do nome.
  detail?: string;
  // Só o ícone: a escolha continua no nome lido e no menu.
  iconOnly?: boolean;
  // Texto à vista quando ele não é o nome lido ("em notebook" para a máquina "notebook").
  shown?: string;
};

// A pílula quieta do app de PC (quiet_pill): 26 de altura, texto 12.5 apagado, ícone 14 e a seta do
// menu. O hitSlop leva o toque aos 44 pt sem engordar o desenho.
export const QuietPill = forwardRef<View, Props>(function QuietPill(
  { icon, label, aria, disabled, actions, menuTitle, onAction, onPress, tone, plain, leading, detail, iconOnly, shown }, ref,
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
      hitSlop={{ top: 9, bottom: 9, left: iconOnly ? 7 : 2, right: iconOnly ? 7 : 2 }}
      accessibilityRole="button"
      accessibilityLabel={`${aria}: ${label}`}
      accessibilityHint={detail}
      accessibilityState={{ disabled: !!disabled }}
      style={[styles.pill, press.style, disabled && styles.disabled]}
    >
      {leading ?? (icon ? <Icon name={icon} size={14} color={ink} /> : null)}
      {iconOnly ? null : <Text style={[styles.text, tone && { color: ink }]} numberOfLines={1}>{shown ?? (detail ? `${label} · ${detail}` : label)}</Text>}
      {plain || iconOnly ? null : <Icon name="ChevronDown" size={12} color={theme.tokens.text.muted} />}
    </AnimatedPressable>
  );
  return (
    <Animated.View entering={FadeIn.duration(220)} style={iconOnly ? undefined : styles.shrink}>
      {body}
      {actions ? <AnchoredPanel open={open && !disabled} anchor={anchor.current} onClose={() => setOpen(false)}
        label={menuTitle ?? aria} closeLabel={m.native_close()} radius={12} padding={4}>
        <SettingsList items={actions.map((it) => ({ id: it.id, label: it.title, hint: it.subtitle,
          selected: it.state === 'on', disabled: it.attributes?.disabled, leading: it.leading, labelColor: it.titleColor }))}
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
