import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { ConnectMark } from '../connect/ConnectHero';
import * as m from '../../paraglide/messages';

/** Topo do "Entrar no Hangar": a marca, o título e a frase que explica o campo único. */
export function LoginHeader({ markSize = 64 }: { markSize?: number }) {
  return <View style={styles.header}>
    <ConnectMark size={markSize} />
    <Text accessibilityRole="header" style={styles.title}>{m.login_hangar_title()}</Text>
    <Text style={styles.subtitle}>{m.login_hangar_subtitle()}</Text>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  header: { alignItems: 'center', gap: theme.base.space[2] },
  title: { ...theme.type.title, color: theme.tokens.text.primary, textAlign: 'center' },
  subtitle: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center', lineHeight: 21 },
}));
