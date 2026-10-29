import { Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { AccountIcon } from '@hangar/core';
import { Icon, type IconName } from '../../ui/Icon';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { ACCOUNT_ICON_NAME } from '../tags/tagVisuals';
import { SPACE } from '../../theme/scale';
import * as m from '../../paraglide/messages';

export type PlateMachine = { label: string; color: string; icon: IconName };
export type PlateAccount = { label: string; name: string; color: string; icon: AccountIcon | null };

// Placa da linha da sessão: [máquina] [conta]. A linha da lista e a animação da novidade desenham esta.
export function SessionPlate({ machine, account, provider }: { machine: PlateMachine | null; account: PlateAccount | null; provider: string }) {
  const { theme } = useUnistyles();
  const muted = theme.tokens.text.muted;
  return (
    <View style={styles.plate}>
      {machine ? (
        <View style={styles.plateSeg}>
          {/* Ícone apagado como o do ramo; a cor da etiqueta fica no nome. */}
          <View style={styles.serverIcon} accessibilityElementsHidden importantForAccessibility="no">
            <Icon name={machine.icon} size={SPACE[3]} color={muted} />
          </View>
          <Text style={[styles.meta, styles.metaRest, { color: machine.color }]} numberOfLines={1}>{machine.label}</Text>
        </View>
      ) : null}
      {account ? (
        <View style={[styles.plateSeg, styles.account]} accessibilityLabel={m.sessao_conta({ n: account.name })}>
          {account.icon === 'provider' ? <ProviderGlyph provider={provider} size={SPACE[3]} inline color={account.color} />
            : account.icon ? <Icon name={ACCOUNT_ICON_NAME[account.icon]} size={SPACE[3]} color={account.color} />
            : <View style={[styles.accountDot, { backgroundColor: account.color }]} />}
          <Text style={[styles.accountText, { color: theme.tokens.text.secondary }]} numberOfLines={1}>{account.label}</Text>
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  // A placa ocupa no máximo 60% da linha; dentro dela a conta guarda o nome curto e a máquina encolhe.
  plate: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minWidth: 0, maxWidth: '60%', flexShrink: 0, paddingLeft: theme.base.space[2] },
  plateSeg: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minWidth: 0, flexShrink: 1 },
  account: { maxWidth: 96, flexShrink: 0 },
  accountDot: { width: 5, height: 5, borderRadius: theme.base.radius.full, flexShrink: 0 },
  accountText: { fontSize: theme.base.text.xxs, fontWeight: theme.base.weight.medium, minWidth: 0, flexShrink: 1 },
  serverIcon: { flexShrink: 0 },
  meta: { fontSize: theme.base.text.xs, lineHeight: 18 },
  metaRest: { flexShrink: 1 },
}));
