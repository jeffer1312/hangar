import { useMemo } from 'react';
import { View, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { UnistylesThemes } from 'react-native-unistyles';

// Tema completo do unistyles (tokens + base) — UnistylesTheme não é exportado na raiz.
type TemaApp = UnistylesThemes[keyof UnistylesThemes];
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import type { MarkdownStyle } from 'react-native-enriched-markdown';

// Tema do markdown mapeado dos tokens do app (cores/links/code) — a lib recebe um
// MarkdownStyle plano; sem ele usa defaults pretos que somem no tema escuro.
export function mkMarkdownStyle(t: TemaApp): MarkdownStyle {
  return {
    paragraph: { color: t.tokens.text.primary, fontSize: t.base.text.base },
    h1: { color: t.tokens.text.primary, fontSize: t.base.text.xl, fontWeight: '700' },
    h2: { color: t.tokens.text.primary, fontSize: t.base.text.lg, fontWeight: '700' },
    h3: { color: t.tokens.text.primary, fontSize: t.base.text.base, fontWeight: '600' },
    h4: { color: t.tokens.text.primary, fontSize: t.base.text.base, fontWeight: '600' },
    h5: { color: t.tokens.text.primary, fontSize: t.base.text.sm, fontWeight: '600' },
    h6: { color: t.tokens.text.secondary, fontSize: t.base.text.sm, fontWeight: '600' },
    link: { color: t.tokens.accent.base, underline: true },
    strong: { color: t.tokens.text.primary },
    em: { color: t.tokens.text.primary },
    code: {
      fontFamily: t.base.fontMono,
      fontSize: t.base.text.sm,
      color: t.tokens.text.primary,
      backgroundColor: t.tokens.bg.hover,
    },
    codeBlock: {
      backgroundColor: t.tokens.bg.surface,
      borderColor: t.tokens.border.subtle,
      borderWidth: 1,
      borderRadius: t.base.radius.sm,
      padding: t.base.space[2],
    },
    blockquote: { borderColor: t.tokens.border.default, backgroundColor: 'transparent', color: t.tokens.text.secondary },
    list: { bulletColor: t.tokens.text.muted, color: t.tokens.text.primary },
    table: {
      headerBackgroundColor: t.tokens.bg.elevated,
      headerTextColor: t.tokens.text.primary,
      borderColor: t.tokens.border.subtle,
      borderWidth: 1,
      color: t.tokens.text.primary,
    },
  };
}

export function AssistantBubble({ text }: { text: string }) {
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
    // fundo sutil pra bolha se distinguir do papel sem virar card pesado (PWA: vidro leve)
    backgroundColor: theme.tokens.bg.elevated,
    borderRadius: theme.base.radius.lg,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
  },
}));
