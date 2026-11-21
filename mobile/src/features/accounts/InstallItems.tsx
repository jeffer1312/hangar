import { ActivityIndicator, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from '../../ui/Icon';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

/** Etapas com o estado de cada uma; `labels` troca as da instalação (a atualização tem três). */
export function InstallItems({ at, failed, labels }: { at: number; failed: boolean; labels?: string[] }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const items = labels ?? [m.mobile_cliproxy_wizard_item_download(), m.mobile_cliproxy_wizard_item_key(),
    m.mobile_cliproxy_wizard_item_service(), m.mobile_cliproxy_wizard_item_check()];
  return <View style={styles.items}>
    {items.map((label, i) => {
      const done = i < at;
      const now = i === at && !failed;
      return <View key={label} style={styles.item}>
        <View style={[styles.itemDot, { borderColor: done ? theme.tokens.status.success : c.borderStrong },
          done && { backgroundColor: theme.tokens.status.success }]}>
          {done ? <Icon name="Check" size={11} color={theme.tokens.text.inverse} /> : now ? <ActivityIndicator size="small" color={c.accent} /> : null}
        </View>
        <Text style={[styles.itemText, { color: done || now ? c.text : c.muted }]}>{label}</Text>
      </View>;
    })}
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  items: { gap: theme.base.space[3], marginVertical: theme.base.space[1] },
  item: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  itemDot: { width: 20, height: 20, borderRadius: theme.base.radius.full, borderWidth: 1.5, alignItems: 'center', justifyContent: 'center' },
  itemText: { fontSize: theme.base.text.sm },
}));
