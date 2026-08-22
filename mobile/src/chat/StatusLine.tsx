import { Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';

// Linha de status do agente (status_line cru do pane): mono, uma linha, como o rodapé da PWA.
export function StatusLine({ line }: { line: string | null }) {
  const { theme } = useUnistyles();
  if (!line) return null;
  return (
    <View style={styles.wrap}>
      <Text style={[styles.txt, { color: theme.tokens.text.muted }]} numberOfLines={1}>
        {line}
      </Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    alignSelf: 'stretch',
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[1],
  },
  txt: {
    fontSize: theme.base.text.xxs,
    fontFamily: theme.base.fontMono,
  },
}));
