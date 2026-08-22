import { useMemo } from 'react';
import { View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { mkMarkdownStyle } from './AssistantBubble';

// A bolha translúcida da PWA (prévia em voo): mesmo markdown do assistente, opacidade 0.7.
export function PreviewBubble({ text }: { text: string }) {
  const { theme } = useUnistyles();
  const md = useMemo(() => mkMarkdownStyle(theme), [theme]);
  return (
    <View style={styles.wrap}>
      <EnrichedMarkdownText markdown={text} markdownStyle={md} flavor="github" />
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    alignSelf: 'flex-start',
    maxWidth: '92%',
    backgroundColor: theme.tokens.bg.elevated,
    borderRadius: theme.base.radius.lg,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    opacity: 0.7,
  },
}));
