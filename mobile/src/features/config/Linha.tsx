import type { ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon, type IconName } from '../../ui/Icon';
import { superficie } from '../../theme/superficie';

type Props = {
  titulo: string;
  descricao?: string;
  icon?: IconName;
  onPress?: () => void;
  onLongPress?: () => void;
  /** Controle da linha (segmentado, slider, paleta). Sem ele e com `onPress`, a linha ganha chevron. */
  children?: ReactNode;
  /** Peça à direita do título, na mesma linha (ponto de estado, valor, botão pequeno). */
  direita?: ReactNode;
};

export function Linha({ titulo, descricao, icon, onPress, onLongPress, children, direita }: Props) {
  const { theme } = useUnistyles();
  const corpo = (
    <>
      <View style={styles.cabeca}>
        {icon ? <Icon name={icon} size={20} color={theme.tokens.accent.base} /> : null}
        <View style={styles.textos}>
          <Text style={styles.titulo}>{titulo}</Text>
          {descricao ? <Text style={styles.descricao}>{descricao}</Text> : null}
        </View>
        {direita}
        {onPress && !children ? <Icon name="ChevronRight" size={18} /> : null}
      </View>
      {children}
    </>
  );
  if (!onPress && !onLongPress) return <View style={styles.linha}>{corpo}</View>;
  return (
    <Pressable
      onPress={onPress}
      onLongPress={onLongPress}
      accessibilityRole="button"
      accessibilityLabel={titulo}
      style={({ pressed }) => [styles.linha, pressed && styles.tocada]}
    >
      {corpo}
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  linha: {
    backgroundColor: superficie(theme, 0.6),
    borderRadius: theme.base.radius.lg,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[3],
    gap: theme.base.space[3],
    minHeight: 56,
    justifyContent: 'center',
  },
  // Realce de estado é tinta por cima da linha, não superfície: cor chapada aqui é de propósito.
  tocada: { backgroundColor: theme.tokens.bg.hover },
  cabeca: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  textos: { flex: 1, gap: 2 },
  titulo: { fontSize: theme.base.text.base, color: theme.tokens.text.primary, fontWeight: '500' },
  descricao: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
}));
