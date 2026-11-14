import type { ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Icon, type IconName } from '../../ui/Icon';
import { useSettingsColors } from './colors';

// O controle de baixo recua até o texto: caixa do ícone + o vão da linha.
const ICON_BOX = 36;

type Props = {
  title: string;
  description?: string;
  /** Ícone na caixinha à esquerda; sem ele, a linha usa o ícone da seção (line_with do Rust). */
  icon?: IconName;
  onPress?: () => void;
  onLongPress?: () => void;
  /** Controle da linha (segmentado, slider, cores): no celular desce para baixo do título. */
  children?: ReactNode;
  /** Peça à direita do título, na mesma linha (ponto de estado, botão pequeno). */
  right?: ReactNode;
  /** Peças na linha do título, antes e depois dele (bolinha de não visto, selo). */
  titleBefore?: ReactNode;
  titleAfter?: ReactNode;
  /** Rótulo da linha (com ou sem toque) quando o título sozinho não diz tudo (o selo fica fora do texto). */
  accessibilityLabel?: string;
};

// O Pressable envolve só o bloco ícone+título, nunca a linha inteira: um Pressable com rótulo
// próprio funde tudo que está dentro dele num nó só de acessibilidade, e o botão de `right`
// ou do controle some pro leitor de tela.
export function SettingsRow({ title, description, icon, onPress, onLongPress, children, right, titleBefore, titleAfter, accessibilityLabel }: Props) {
  const c = useSettingsColors();
  const inLine = !!(titleBefore || titleAfter);
  const titleText = <Text style={[icon ? styles.title : styles.titleSmall, inLine && styles.titleShrink, { color: c.text }]}>{title}</Text>;
  const head = (
    <>
      {icon ? (
        <View style={[styles.iconBox, { borderColor: c.border, backgroundColor: c.inset }]}>
          <Icon name={icon} size={16} color={c.muted} />
        </View>
      ) : null}
      <View style={styles.texts}>
        {inLine
          ? <View style={styles.titleLine}>{titleBefore}{titleText}{titleAfter}</View>
          : titleText}
        {description ? (
          <Text style={[icon ? styles.desc : styles.descSmall, { color: c.muted }]}>{description}</Text>
        ) : null}
      </View>
      {/* Chevron só na linha que é pura navegação: com controle ou botão próprio ele viraria enfeite ambíguo. */}
      {onPress && !children && !right ? <Icon name="ChevronRight" size={18} color={c.faint} /> : null}
    </>
  );
  return (
    <View style={[styles.row, { borderTopColor: c.border }]}>
      <View style={styles.headLine}>
        {onPress || onLongPress ? (
          <Pressable
            onPress={onPress}
            onLongPress={onLongPress}
            accessibilityRole="button"
            accessibilityLabel={accessibilityLabel ?? title}
            style={({ pressed }) => [styles.head, pressed && { opacity: 0.6 }]}
          >
            {head}
          </Pressable>
        ) : (
          // Sem toque, o rótulo dado ainda vale: o bloco ícone+título vira um nó só com ele.
          <View style={styles.head} accessible={accessibilityLabel ? true : undefined} accessibilityLabel={accessibilityLabel}>{head}</View>
        )}
        {right}
      </View>
      {children ? <View style={icon ? styles.controlIndented : undefined}>{children}</View> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  row: { borderTopWidth: 1, paddingVertical: theme.base.space[3], paddingHorizontal: theme.base.space[4], gap: theme.base.space[2] },
  headLine: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  head: { flex: 1, minHeight: 36, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  iconBox: { width: ICON_BOX, height: ICON_BOX, borderRadius: theme.base.radius.sm, borderWidth: 1, alignItems: 'center', justifyContent: 'center' },
  texts: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  desc: { fontSize: theme.base.text.xs },
  titleSmall: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  titleShrink: { flexShrink: 1 },
  titleLine: { flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: theme.base.space[1.5] },
  descSmall: { fontSize: theme.base.text.xs },
  controlIndented: { paddingLeft: ICON_BOX + theme.base.space[3] },
}));
