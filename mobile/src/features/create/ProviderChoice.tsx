import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { CREATE_SESSION_PROVIDERS, providerName, type Provider } from '@hangar/core';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

type Probe = Record<string, { disponivel: boolean; motivo?: string | null }>;

// Sem sonda (falhou ou não chegou) todos ficam oferecidos: o servidor recusa o que faltar.
const unavailable = (probe: Probe | null | undefined, p: string) => probe?.[p]?.disponivel === false;

function reason(p: Provider, motivo: string | null | undefined) {
  return motivo == null || motivo === 'nao_encontrado' ? m.native_create_provider_missing({ p: providerName(p) })
    : motivo === 'sem_permissao' ? m.native_create_provider_execution_denied({ p: providerName(p) }) : motivo;
}

// O motivo de cada agente apagado continua à vista, embaixo da escolha.
function Reasons({ probe }: { probe: Probe | null | undefined }) {
  const off = CREATE_SESSION_PROVIDERS.filter((p) => unavailable(probe, p));
  return off.length ? <View style={styles.reasons}>
    {off.map((p) => <Text key={p} style={styles.reason}>{reason(p, probe?.[p]?.motivo)}</Text>)}
  </View> : null;
}

// Tocar no agente já marcado também passa adiante: confirma a escolha antes de a sonda responder.
const pick = (value: Provider, p: Provider, onChange: (p: Provider) => void) => {
  if (p !== value) haptic.selection();
  onChange(p);
};

/** As abas de agente do menu de modelo, como o app de PC: só o logo, o escolhido aceso. */
export function ProviderLogoTabs({ value, onChange, probe, disabled = false }: {
  value: Provider; onChange: (p: Provider) => void; probe?: Probe | null; disabled?: boolean;
}) {
  const { theme } = useUnistyles();
  return <View>
    <View style={styles.tabs} accessibilityRole="radiogroup" accessibilityLabel={m.native_create_provider_aria()}>
      {CREATE_SESSION_PROVIDERS.map((p) => {
        const on = p === value;
        const off = disabled || unavailable(probe, p);
        return <Pressable key={p} disabled={off} onPress={() => pick(value, p, onChange)}
          style={({ pressed }) => [styles.tab, { backgroundColor: on ? theme.tokens.accent.dim : pressed ? theme.tokens.fillSubtle : 'transparent' },
            off && styles.off]}
          accessibilityRole="radio" accessibilityLabel={providerName(p)}
          accessibilityState={{ selected: on, checked: on, disabled: off }}>
          <ProviderGlyph provider={p} size={18} inline />
        </Pressable>;
      })}
    </View>
    <Reasons probe={probe} />
  </View>;
}

/** O seletor de agente do formulário: controle segmentado com logo e nome, em duas linhas de três. */
export function ProviderTrack({ value, onChange, probe, disabled = false }: {
  value: Provider; onChange: (p: Provider) => void; probe?: Probe | null; disabled?: boolean;
}) {
  const { theme } = useUnistyles();
  return <View style={styles.trackWrap}>
    <View style={styles.track} accessibilityRole="radiogroup" accessibilityLabel={m.native_create_provider_aria()}>
      {CREATE_SESSION_PROVIDERS.map((p) => {
        const on = p === value;
        const off = disabled || unavailable(probe, p);
        return <Pressable key={p} disabled={off} onPress={() => pick(value, p, onChange)}
          style={({ pressed }) => [styles.segment, { backgroundColor: on ? theme.tokens.accent.dim : pressed ? theme.tokens.bg.hover : 'transparent' },
            off && styles.off]}
          accessibilityRole="radio" accessibilityLabel={providerName(p)}
          accessibilityState={{ selected: on, checked: on, disabled: off }}>
          <ProviderGlyph provider={p} size={16} inline />
          <Text style={[styles.segmentText, on && styles.segmentOn]} numberOfLines={1}>{providerName(p)}</Text>
        </Pressable>;
      })}
    </View>
    <Reasons probe={probe} />
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  tabs: { flexDirection: 'row', gap: theme.base.space[0.5], paddingHorizontal: theme.base.space[1], paddingBottom: theme.base.space[1] },
  tab: { flex: 1, minWidth: 0, minHeight: 44, alignItems: 'center', justifyContent: 'center', borderRadius: theme.base.radius.sm },
  off: { opacity: 0.4 },
  reasons: { paddingHorizontal: theme.base.space[2], paddingBottom: theme.base.space[1], gap: theme.base.space[0.5] },
  reason: { fontSize: theme.base.text.xxs, color: theme.tokens.text.muted },
  trackWrap: { gap: theme.base.space[1] },
  track: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[0.5], padding: theme.base.space[1],
    borderRadius: theme.base.radius.md, backgroundColor: theme.tokens.fillSubtle },
  // Três por linha: o vão entre eles sai da base de 33%.
  segment: { flexBasis: '32%', flexGrow: 1, minWidth: 0, minHeight: 44, flexDirection: 'row', alignItems: 'center', justifyContent: 'center',
    gap: theme.base.space[1.5], paddingHorizontal: theme.base.space[1], borderRadius: theme.base.radius.sm },
  segmentText: { flexShrink: 1, fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  segmentOn: { color: theme.tokens.text.primary, fontWeight: theme.base.weight.medium },
}));
