import { Text, View } from 'react-native';
import { SvgXml } from 'react-native-svg';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { IntegrationBrand, IntegrationStatus } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { useSettingsColors } from '../config/colors';
import { brandLogoXml } from './brandLogos';

/** Ícone da integração: logo do serviço sobre a cor da marca; sem marca, a inicial do nome, ou o terminal se for local.
 * Com `status`, a bolinha no canto repete o estado que a linha também diz em texto. */
export function IntegrationIcon({ brand, name, local, status }: {
  brand?: IntegrationBrand | null;
  name: string;
  local?: boolean;
  status?: IntegrationStatus;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const xml = brand ? brandLogoXml(brand.icon) : null;
  const dot = status === 'ready' ? theme.tokens.status.success
    : status === 'auth_required' || status === 'auth_expired' ? theme.tokens.status.warning
      : status === 'error' ? theme.tokens.status.error
        : status === 'disabled' ? c.faint : null;
  const initial = (name.match(/[\p{L}\p{N}]/u)?.[0] ?? '').toUpperCase();
  return (
    <View style={styles.wrap} accessible={false} importantForAccessibility="no-hide-descendants">
      {xml && brand ? (
        <View style={[styles.tile, { backgroundColor: brand.color }]}>
          <SvgXml xml={xml} width={20} height={20} />
        </View>
      ) : local ? (
        <View style={[styles.tile, { backgroundColor: c.hover }]}>
          <Icon name="SquareTerminal" size={18} color={c.muted} />
        </View>
      ) : (
        <View style={[styles.tile, { backgroundColor: c.hover }]}>
          <Text style={[styles.initial, { color: c.accent }]}>{initial}</Text>
        </View>
      )}
      {dot ? <View style={[styles.dot, { backgroundColor: dot, borderColor: c.border }]} /> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: { width: 36, height: 36 },
  tile: { width: 36, height: 36, borderRadius: 9, alignItems: 'center', justifyContent: 'center' },
  initial: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  dot: { position: 'absolute', right: -3, bottom: -3, width: 12, height: 12, borderRadius: 6, borderWidth: 2 },
}));
