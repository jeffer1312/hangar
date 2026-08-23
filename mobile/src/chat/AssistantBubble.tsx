import { useEffect, useMemo, useState } from 'react';
import { Pressable, View, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { UnistylesThemes } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import type { MarkdownStyle } from 'react-native-enriched-markdown';
import { fileUrlNative, fileAuthHeader, lerTabelaMarkdown, parseFilePaths } from '@hangar/core';
import * as m from '../paraglide/messages';
import { TableChart } from './TableChart';
import { getTableChartPref, setTableChartPref } from './tableChartPref';

// Tema completo do unistyles (tokens + base) — UnistylesTheme não é exportado na raiz.
type TemaApp = UnistylesThemes[keyof UnistylesThemes];

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
      color: t.tokens.text.primary,
      backgroundColor: t.tokens.bg.surface,
      borderColor: t.tokens.border.subtle,
      borderWidth: 1,
      borderRadius: t.base.radius.sm,
      padding: t.base.space[2],
      syntaxColors: {
        keyword: t.tokens.accent.base,
        string: t.tokens.status.success,
        number: t.tokens.status.warning,
        constant: t.tokens.status.warning,
        function: t.tokens.accent.base,
        type: t.tokens.accent.base,
        property: t.tokens.text.secondary,
        tag: t.tokens.accent.base,
        attribute: t.tokens.text.secondary,
        comment: t.tokens.text.muted,
      },
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

export function AssistantBubble({ text, sessionName }: { text: string; sessionName?: string }) {
  const { theme } = useUnistyles();
  const md = useMemo(() => mkMarkdownStyle(theme), [theme]);
  const refs = useMemo(() => parseFilePaths(text), [text]);
  const hasRefs = refs.length > 0 && !!sessionName;
  const tabelas = useMemo(() => lerTabelaMarkdown(text), [text]);
  const [pref, setPref] = useState(() => getTableChartPref());
  const [colIndices, setColIndices] = useState<number[]>(() => tabelas.map(() => 0));
  useEffect(() => {
    if (colIndices.length !== tabelas.length) setColIndices(tabelas.map(() => 0));
  }, [tabelas.length, colIndices.length]);

  return (
    <View style={styles.wrap}>
      <EnrichedMarkdownText markdown={text} markdownStyle={md} flavor="github" />
      {tabelas.length > 0 ? (
        <View style={styles.tableBlock}>
          <Pressable
            onPress={() => {
              const next = pref === 'chart' ? 'table' : 'chart';
              setPref(next);
              setTableChartPref(next);
            }}
            style={[styles.chartBtn, { borderColor: theme.tokens.border.subtle, backgroundColor: theme.tokens.bg.surface }]}
            accessibilityRole="button"
          >
            <Text style={[styles.chartBtnText, { color: theme.tokens.accent.base }]}>
              {pref === 'chart' ? m.tabela_botao_tabela() : m.tabela_botao_grafico()}
            </Text>
          </Pressable>
          {pref === 'chart'
            ? tabelas.map((t, idx) => (
                <TableChart
                  key={idx}
                  tabela={t}
                  coluna={colIndices[idx] ?? 0}
                  onColuna={(c) =>
                    setColIndices((prev) => {
                      const copy = [...prev];
                      copy[idx] = c;
                      return copy;
                    })
                  }
                />
              ))
            : null}
        </View>
      ) : null}
      {hasRefs ? (
        <View style={styles.atts}>
          {refs.map((r) => {
            const isImg = r.kind === 'image';
            const uri = r.url ?? fileUrlNative(sessionName!, r.path);
            const headers = r.url ? undefined : fileAuthHeader();
            if (isImg) {
              return <Image key={r.path} source={{ uri, headers }} style={styles.thumb} contentFit="cover" transition={150} />;
            }
            const icon = r.kind === 'pdf' ? '📄' : r.kind === 'html' ? '🌐' : r.kind === 'audio' ? '🎵' : '📎';
            return (
              <View key={r.path} style={[styles.chip, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.subtle }]}>
                <Text style={styles.chipIco}>{icon}</Text>
                <Text style={[styles.chipName, { color: theme.tokens.text.primary }]} numberOfLines={1}>
                  {r.name}
                </Text>
              </View>
            );
          })}
        </View>
      ) : null}
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
    gap: theme.base.space[2],
  },
  atts: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: theme.base.space[2],
    marginTop: theme.base.space[1],
  },
  thumb: {
    width: 96,
    height: 96,
    borderRadius: theme.base.radius.md,
    backgroundColor: theme.tokens.bg.surface,
  },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1],
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1],
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    maxWidth: 220,
  },
  chipIco: {
    fontSize: 16,
  },
  chipName: {
    fontSize: theme.base.text.xs,
    flexShrink: 1,
  },
  tableBlock: {
    gap: theme.base.space[2],
    marginTop: theme.base.space[1],
  },
  chartBtn: {
    alignSelf: 'flex-start',
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[1],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
  },
  chartBtnText: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
}));
