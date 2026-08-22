import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { providerName } from '@hangar/core';
import type { Provider } from '@hangar/core';

const PROVIDERS: Provider[] = ['claude', 'codex', 'pi', 'kimi'];

export function ProviderPicker({
  value,
  onChange,
  disabledMap,
}: {
  value: Provider;
  onChange: (p: Provider) => void;
  disabledMap?: Record<string, { disponivel: boolean }>;
}) {
  return (
    <View style={styles.grid} accessibilityRole="radiogroup">
      {PROVIDERS.map((p) => {
        const dis = disabledMap?.[p] && !disabledMap[p].disponivel;
        const sel = value === p;
        return (
          <Pressable
            key={p}
            onPress={() => !dis && onChange(p)}
            disabled={!!dis}
            style={[styles.tile, sel && styles.tileOn, dis && styles.tileDis]}
            accessibilityRole="button"
            accessibilityState={{ selected: sel, disabled: !!dis }}
            accessibilityLabel={providerName(p)}
          >
            <Text style={[styles.txt, sel && styles.txtOn, dis && styles.txtDis]}>{providerName(p)}</Text>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  grid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: theme.base.space[2],
  },
  tile: {
    minWidth: 104,
    flexGrow: 1,
    flexBasis: '22%',
    height: 44,
    justifyContent: 'center',
    alignItems: 'center',
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    backgroundColor: theme.tokens.bg.surface,
  },
  tileOn: {
    borderColor: theme.tokens.accent.base,
    backgroundColor: theme.tokens.accent.dim,
  },
  tileDis: { opacity: 0.45 },
  txt: {
    fontSize: theme.base.text.sm,
    fontWeight: '500',
    color: theme.tokens.text.secondary,
  },
  txtOn: { color: theme.tokens.text.primary },
  txtDis: { color: theme.tokens.text.muted },
}));
