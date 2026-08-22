import { Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';

// Bolha do usuário: alinhada à direita, cor bubbleUser do tema (espelho do app.css).
export function UserBubble({ text }: { text: string }) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.bubble}>
      <Text style={[styles.txt, { color: theme.tokens.text.primary }]} selectable>
        {text}
      </Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  bubble: {
    alignSelf: 'flex-end',
    maxWidth: '85%',
    backgroundColor: theme.tokens.bubbleUser,
    borderRadius: theme.base.radius.lg,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
  },
  txt: {
    fontSize: theme.base.text.base,
  },
}));
