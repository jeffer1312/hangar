import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { providerName, SESSION_PROVIDERS } from '@hangar/core';
import type { Provider } from '@hangar/core';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';

export function ProviderPicker({
  value,
  onChange,
  disabledMap,
  providers: PROVIDERS = SESSION_PROVIDERS,
}: {
  value: Provider;
  onChange: (p: Provider) => void;
  disabledMap?: Record<string, { disponivel: boolean; motivo?: string | null }>;
  /** A criação de sessão oferece também o Antigravity; a orquestração fica com os de sempre. */
  providers?: readonly Provider[];
}) {
  return (
    <View>
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
    {PROVIDERS.filter((p) => disabledMap?.[p]?.disponivel === false).map((p) => {
      const reason = disabledMap?.[p]?.motivo;
      return <Text key={p} style={styles.reason}>{reason == null || reason === 'nao_encontrado'
        ? m.native_create_provider_missing({ p: providerName(p) })
        : reason === 'sem_permissao' ? m.native_create_provider_execution_denied({ p: providerName(p) }) : reason}</Text>;
    })}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  reason: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted, paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[1] },
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
    backgroundColor: superficie(theme),
  },
  tileOn: {
    borderColor: theme.tokens.accent.base,
    backgroundColor: theme.tokens.accent.dim,
  },
  tileDis: { opacity: 0.45 },
  txt: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.medium,
    color: theme.tokens.text.secondary,
  },
  txtOn: { color: theme.tokens.text.primary },
  txtDis: { color: theme.tokens.text.muted },
}));