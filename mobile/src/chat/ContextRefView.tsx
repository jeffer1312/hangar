import { useMemo, useState } from 'react';
import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { EnrichedMarkdownText, type MarkdownStyle } from 'react-native-enriched-markdown';
import { codeLabel, contextRefLabel, parseContextRefs, type ContextRef, type QuoteRef } from '@hangar/core';
import * as m from '../paraglide/messages';
import { Icon } from '../ui/Icon';
import { mkMutedMarkdownStyle } from './AssistantBubble';

// Mensagem do usuário com blocos <hangar-ref>: a citação em destaque (barra à esquerda) com o
// comentário fora dela, o elemento como cartão compacto, as linhas de arquivo com o nome, as linhas e o
// código em fonte monoespaçada, e o texto digitado como sempre.
export function ContextRefView({ text, onQuoteClick }: { text: string; onQuoteClick?: (ref: QuoteRef) => void }) {
  const { theme } = useUnistyles();
  const segments = useMemo(() => parseContextRefs(text), [text]);
  // O trecho do nativo é a fonte Markdown; o do app e da web, o texto da tela. Os dois passam pelo
  // markdown para `**` nunca aparecer cru. Um estilo só para todas as citações da mensagem.
  const md = useMemo(() => mkMutedMarkdownStyle(theme), [theme]);
  return (
    <View style={styles.col}>
      {segments.map((seg, i) => ('ref' in seg
        ? <RefItem key={i} refItem={seg.ref} md={md} onQuoteClick={onQuoteClick} />
        : seg.text.trim() ? <Text key={i} style={styles.txt}>{seg.text.trim()}</Text> : null))}
    </View>
  );
}

function RefItem({ refItem, md, onQuoteClick }: { refItem: ContextRef; md: MarkdownStyle; onQuoteClick?: (ref: QuoteRef) => void }) {
  const { theme } = useUnistyles();
  const [html, setHtml] = useState(false);
  const comment = refItem.comment.trim();
  return (
    <>
      {refItem.kind === 'quote' ? (
        <View style={[styles.quote, { borderColor: theme.tokens.accent.base }]}>
          {onQuoteClick && refItem.source ? (
            <Pressable
              onPress={() => onQuoteClick(refItem)}
              hitSlop={6}
              style={styles.head}
              accessibilityRole="button"
              accessibilityLabel={m.cite_go_to_source()}
            >
              <Text style={[styles.headTxt, { color: theme.tokens.accent.base }]}>{m.cite_quoted()}</Text>
              <Icon name="CornerLeftUp" size={14} color={theme.tokens.accent.base} />
            </Pressable>
          ) : (
            <Text style={[styles.headTxt, { color: theme.tokens.accent.base }]}>{m.cite_quoted()}</Text>
          )}
          <EnrichedMarkdownText markdown={refItem.text} markdownStyle={md} flavor="github" />
        </View>
      ) : refItem.kind === 'code' ? (
        <View style={[styles.quote, { borderColor: theme.tokens.accent.base }]}>
          <Text style={[styles.headTxt, { color: theme.tokens.accent.base }]}>{m.cite_code()}</Text>
          <Text style={[styles.codeName, { color: theme.tokens.text.primary }]} numberOfLines={1}>{codeLabel(refItem)}</Text>
          {codeLabel(refItem).startsWith(refItem.path)
            ? null
            : <Text style={[styles.url, { color: theme.tokens.text.muted }]} numberOfLines={1}>{refItem.path}</Text>}
          <ScrollView horizontal showsHorizontalScrollIndicator={false}>
            <Text style={[styles.code, { color: theme.tokens.text.secondary }]} selectable>{refItem.text}</Text>
          </ScrollView>
        </View>
      ) : (
        <View style={[styles.card, { borderColor: theme.tokens.border.subtle }]}>
          <Text style={[styles.headTxt, { color: theme.tokens.accent.base }]}>{m.cite_element()}</Text>
          <Text style={[styles.name, { color: theme.tokens.text.primary }]}>{contextRefLabel({ ...refItem, comment: '' })}</Text>
          {refItem.url ? <Text style={[styles.url, { color: theme.tokens.text.muted }]} numberOfLines={1}>{refItem.url}</Text> : null}
          {refItem.html ? (
            <>
              <Pressable onPress={() => setHtml((v) => !v)} hitSlop={6} style={styles.toggle} accessibilityRole="button" accessibilityState={{ expanded: html }}>
                <Text style={[styles.toggleTxt, { color: theme.tokens.text.secondary }]}>{m.cite_element_html()}</Text>
              </Pressable>
              {html ? <Text style={[styles.code, { color: theme.tokens.text.secondary }]} selectable>{refItem.html}</Text> : null}
            </>
          ) : null}
        </View>
      )}
      {comment ? <Text style={styles.txt}>{comment}</Text> : null}
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  col: { gap: theme.base.space[2] },
  txt: { fontSize: theme.base.text.base * theme.conversa.texto, color: theme.tokens.text.primary },
  quote: { borderLeftWidth: 3, paddingLeft: theme.base.space[2], gap: theme.base.space[0.5] },
  head: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minHeight: 28 },
  headTxt: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold },
  card: { borderWidth: 1, borderRadius: theme.base.radius.md, padding: theme.base.space[2], gap: theme.base.space[0.5] },
  name: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  url: { fontSize: theme.base.text.xs },
  toggle: { minHeight: 32, justifyContent: 'center' },
  toggleTxt: { fontSize: theme.base.text.xs },
  code: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs },
  codeName: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
}));
