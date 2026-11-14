import { Text } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';

export function PatchLine({ line }: { line: string }) {
  return <Text selectable style={[styles.line, line.startsWith('+') ? styles.add : line.startsWith('-') ? styles.remove : null]}>{line || ' '}</Text>;
}
const styles = StyleSheet.create((theme) => ({
  line: { ...theme.type.mono, color: theme.tokens.text.primary, paddingHorizontal: theme.base.space[2] },
  add: { color: theme.tokens.status.success }, remove: { color: theme.tokens.status.error },
}));
