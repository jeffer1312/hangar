import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { superficie } from '../../theme/superficie';

export interface Opcao<T extends string> {
  v: T;
  label: string;
  /** Rótulo de leitor de tela quando o texto curto do botão não explica a opção. */
  aria?: string;
}

export function Segmentado<T extends string>({
  opcoes,
  valor,
  onChange,
}: {
  opcoes: ReadonlyArray<Opcao<T>>;
  valor: T;
  onChange: (v: T) => void;
}) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.trilho}>
      {opcoes.map((o) => {
        const sel = o.v === valor;
        return (
          <Pressable
            key={o.v}
            onPress={() => onChange(o.v)}
            style={[styles.item, sel && { backgroundColor: theme.tokens.accent.dim }]}
            accessibilityRole="button"
            accessibilityLabel={o.aria ?? o.label}
            accessibilityState={{ selected: sel }}
          >
            <Text
              style={[styles.txt, { color: sel ? theme.tokens.accent.base : theme.tokens.text.secondary }]}
              numberOfLines={1}
            >
              {o.label}
            </Text>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  trilho: {
    flexDirection: 'row',
    gap: theme.base.space[1],
    padding: 3,
    borderRadius: theme.base.radius.md,
    backgroundColor: superficie(theme, 0.4),
  },
  item: {
    flex: 1,
    minHeight: 36,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.xs,
  },
  txt: { fontSize: theme.base.text.sm, fontWeight: '600' },
}));
