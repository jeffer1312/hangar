import type { ReactNode } from 'react';
import { ActivityIndicator, Text, View, type LayoutChangeEvent } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon, type IconName } from '../../ui/Icon';
import { superficie } from '../../theme/superficie';
import { useSettingsColors, withAlpha } from './colors';

/**
 * Cartão de seção das Configurações (section_head + settings_box do Rust): quadradinho de destaque
 * com o ícone, título e a linha que explica. Sem `icon`/`title` é só a caixa, para linhas com ícone próprio.
 */
export function SectionCard({
  icon,
  title,
  subtitle,
  extra,
  children,
  onLayout,
  tone,
  busy = false,
}: {
  icon?: IconName;
  title?: string;
  subtitle?: string;
  /** Peça à direita do título (pílula de ação). */
  extra?: ReactNode;
  children?: ReactNode;
  onLayout?: (e: LayoutChangeEvent) => void;
  /** `danger`: o quadradinho do ícone na cor de erro, para o cartão que conta uma falha. `warning`: borda e ícone de
   *  aviso, para o que espera uma resposta da pessoa. */
  tone?: 'danger' | 'warning';
  /** Girando no lugar do ícone: algo em andamento. */
  busy?: boolean;
}) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const danger = tone === 'danger';
  const toneColor = danger ? theme.tokens.status.error : tone === 'warning' ? theme.tokens.status.warning : null;
  return (
    <View
      onLayout={onLayout}
      style={[styles.box, { borderColor: tone === 'warning' ? withAlpha(theme.tokens.status.warning, 0.5) : c.border, backgroundColor: superficie(theme, 0.6) }]}
    >
      {icon && title ? (
        <View style={styles.head}>
          <View style={[styles.square, { backgroundColor: toneColor ? withAlpha(toneColor, 0.14) : c.accentDim }]}>
            {busy ? <ActivityIndicator size="small" color={c.accent} />
              : <Icon name={icon} size={17} color={toneColor ?? c.accentText} />}
          </View>
          <View style={styles.texts}>
            <Text style={[styles.title, { color: c.text }]} accessibilityRole="header">{title}</Text>
            {subtitle ? <Text style={[styles.subtitle, { color: c.muted }]}>{subtitle}</Text> : null}
          </View>
          {extra}
        </View>
      ) : null}
      {/* Sem cabeçalho a divisória da primeira linha encostaria na borda da caixa: sobe 1px e some. */}
      <View style={icon && title ? undefined : styles.bare}>{children}</View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  box: { borderRadius: theme.base.radius.md, borderWidth: 1, overflow: 'hidden' },
  head: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingVertical: theme.base.space[3], paddingHorizontal: theme.base.space[4] },
  square: { width: 32, height: 32, borderRadius: theme.base.radius.sm, alignItems: 'center', justifyContent: 'center' },
  texts: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  subtitle: { fontSize: theme.base.text.xs },
  bare: { marginTop: -1 },
}));
