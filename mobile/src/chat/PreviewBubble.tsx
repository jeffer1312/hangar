import { useEffect, useMemo, useRef } from 'react';
import { Animated, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { splitTodoBlock } from '@hangar/core';
import { mkMarkdownStyle } from './AssistantBubble';

interface Props {
  text: string;
  // md: prévia que veio do PRÓPRIO agente (sidecar/hook) — markdown incremental.
  // full: bloco inteiro, não raspagem parcial do pane (quando vem do sidecar, md e full são ambos true).
  // Mantida no contrato da Task 1; hoje não muda nada no mobile (sem teto de 10 linhas como na PWA).
  md: boolean;
  full: boolean;
}

// Caret piscando (▍): sempre visível na prévia, em todos os ramos (espelha AssistantBubble.svelte).
function Caret() {
  const op = useRef(new Animated.Value(1)).current;
  useEffect(() => {
    const l = Animated.loop(
      Animated.sequence([
        Animated.timing(op, { toValue: 0, duration: 300, useNativeDriver: true }),
        Animated.timing(op, { toValue: 1, duration: 300, useNativeDriver: true }),
      ]),
    );
    l.start();
    return () => l.stop();
  }, [op]);
  return <Animated.Text style={[styles.caret, { opacity: op }]}>▍</Animated.Text>;
}

// Prévia ao vivo (irmã da AssistantBubble.svelte): markdown quando veio do agente, texto mono
// quando é raspagem do pane. Painel de Todos do TUI separado por splitTodoBlock (core):
// cabeçalho vira markdown, árvore de itens fica num bloco mono.
export function PreviewBubble({ text, md, full }: Props) {
  const { theme } = useUnistyles();
  const todo = useMemo(() => splitTodoBlock(text), [text]);
  const prose = todo ? todo.rest : text;
  const mdStyle = useMemo(() => {
    const base = mkMarkdownStyle(theme);
    // prévia distinguível da bolha commitada: texto secondary
    return { ...base, paragraph: { ...base.paragraph, color: theme.tokens.text.secondary } };
  }, [theme]);

  return (
    <View style={styles.wrap}>
      {todo ? (
        <View style={styles.todo}>
          <EnrichedMarkdownText markdown={todo.head} markdownStyle={mdStyle} flavor="github" />
          <Text style={[styles.todoBody, { color: theme.tokens.text.secondary }]}>
            {todo.body}
          </Text>
          {!prose ? <Caret /> : null}
        </View>
      ) : null}
      {!prose ? null : md ? (
        <>
          <EnrichedMarkdownText markdown={prose} markdownStyle={mdStyle} flavor="github" />
          <Caret />
        </>
      ) : (
        <Text style={[styles.plain, { color: theme.tokens.text.secondary }]}>
          {prose}
          <Caret />
        </Text>
      )}
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
    opacity: 0.85,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderStyle: 'dashed',
    gap: theme.base.space[1],
  },
  plain: {
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.xs,
  },
  todo: {
    alignSelf: 'stretch',
    gap: theme.base.space[1],
  },
  todoBody: {
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.xxs,
  },
  caret: {
    color: theme.tokens.accent.base,
    fontSize: theme.base.text.sm,
  },
}));
