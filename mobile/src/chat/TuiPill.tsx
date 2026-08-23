import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import * as m from '../paraglide/messages';

interface Props {
  serverId: string;
  name: string;
  overlay?: boolean;
  login?: boolean;
}

export function TuiPill({ serverId, name, overlay, login }: Props) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const visible = !!(overlay || login);
  if (!visible) return null;

  const label = login ? m.chat_precisa_login() : m.chat_interacao_tui();
  const a11y = login ? m.chat_abrir_terminal_login() : m.chat_abrir_terminal_interagir();

  return (
    <Pressable
      onPress={() => router.push(`/s/${serverId}/${name}/terminal` as never)}
      style={[styles.pill, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]}
      accessibilityRole="button"
      accessibilityLabel={a11y}
    >
      <View style={[styles.dot, { backgroundColor: theme.tokens.status.warning }]} />
      <Text style={[styles.text, { color: theme.tokens.text.primary }]} numberOfLines={1}>
        {label}
      </Text>
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  pill: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    marginHorizontal: theme.base.space[3],
    marginVertical: theme.base.space[2],
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[3],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
    minHeight: 44,
  },
  dot: {
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  text: {
    fontSize: theme.base.text.sm,
    fontWeight: '500',
    flexShrink: 1,
  },
}));
