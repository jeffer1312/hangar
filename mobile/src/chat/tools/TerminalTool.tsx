import { memo, useMemo, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  extractEdits, extractEditPaths, extractFilePath, hexParaRgb, numberedDiff,
  summarizeToolInput, summarizeToolResult, terminalFileTool, terminalToolName, toolPhase,
  type ChatEvent, type NumberedDiff, type NumberedLine,
} from '@hangar/core';
import * as m from '../../paraglide/messages';
import { useSelectableToggle } from './useSelectableToggle';
import { ExitCodeLine } from './ExitCodeLine';

const MAX_LINES = 400;
const basename = (path: string) => path.split(/[\\/]/).pop() ?? path;
const tint = (color: string, alpha: number) => {
  const rgb = hexParaRgb(color);
  return rgb ? `rgba(${rgb.join(',')},${alpha})` : color;
};

function editSummary(diff: NumberedDiff, created: boolean, file: string): string {
  const { add, del } = diff;
  if (created) return add === 1 ? m.term_gravou_1({ arquivo: file }) : m.term_gravou_n({ n: add, arquivo: file });
  if (!add && !del) return m.editdiff_sem_mudanca();
  const added = add === 1 ? m.term_adicionadas_1() : m.term_adicionadas_n({ n: add });
  if (!del) return added;
  if (!add) return del === 1 ? m.term_so_removidas_1() : m.term_so_removidas_n({ n: del });
  return `${added}, ${del === 1 ? m.term_removidas_1() : m.term_removidas_n({ n: del })}`;
}

function MarkedLine({ line, color }: { line: NumberedLine; color: string }) {
  let from = 0;
  const pieces = line.spans.flatMap(([start, end]) => {
    const prefix = line.text.slice(from, start);
    from = end;
    return [prefix, <Text key={start} style={{ backgroundColor: tint(color, 0.3) }}>{line.text.slice(start, end)}</Text>];
  });
  return <>{pieces}{line.text.slice(from)}</>;
}

/** Cabeçalho e diff expostos como no Terminal do desktop nativo. */
export const TerminalTool = memo(function TerminalTool({ use, result, escrevendo = false, onExpand, onOpen }: {
  use: ChatEvent;
  result?: ChatEvent | null;
  escrevendo?: boolean;
  onExpand?: () => void;
  onOpen?: (tool: ChatEvent) => void;
}) {
  const { theme } = useUnistyles();
  const [open, setOpen] = useState(false);
  const phase = toolPhase(result ?? null);
  const failed = phase === 'error';
  const path = extractFilePath(use.tool_input) || String(use.tool_input?.['notebook_path'] ?? '');
  const arg = terminalFileTool(use.tool_name) && path ? basename(path) : summarizeToolInput(use.tool_name, use.tool_input);
  const name = terminalToolName(use.tool_name);
  const diff = useMemo(() => {
    if (failed) return null;
    const edits = extractEdits(use.tool_name, use.tool_input);
    return edits ? numberedDiff(result?.patch, edits, extractEditPaths(use.tool_name, use.tool_input) ?? undefined) : null;
  }, [use, result?.patch, failed]);
  const created = use.tool_name?.toLowerCase() === 'write' && !result?.patch?.length && diff?.del === 0;
  const limit = created ? 10 : 40;
  const total = diff?.hunks.reduce((n, hunk) => n + hunk.length, 0) ?? 0;
  const expandable = !diff || total > limit;
  const dot = failed ? theme.tokens.status.error : phase === 'pending' ? theme.tokens.accent.base : theme.tokens.status.success;
  const outcome = diff && result ? editSummary(diff, created, arg)
    : escrevendo ? m.tool_fase_escrevendo() : phase === 'pending' ? m.estado_em_execucao() : summarizeToolResult(result, use.tool_name);
  const toggle = () => { if (!open) onExpand?.(); setOpen((value) => !value); };
  const bodyPress = useSelectableToggle(toggle);
  const header = <View style={styles.header}>
    <View style={[styles.dot, { backgroundColor: dot }]} />
    <Text style={styles.mono}><Text style={styles.name}>{name}</Text>{`(${arg})`}</Text>
  </View>;
  let remaining = open ? MAX_LINES : limit;
  return (
    <View>
      {expandable ? (
        <Pressable onPress={toggle} style={styles.control} accessibilityRole="button"
          accessibilityLabel={`${name}: ${path || arg}. ${outcome}`} accessibilityState={{ expanded: open }}>
          {header}
        </Pressable>
      ) : header}
      <View style={styles.outcome}>
        <Text style={styles.muted}>⎿</Text>
        <Text style={[styles.mono, { flex: 1, color: failed ? theme.tokens.status.error : theme.tokens.text.secondary }]}>{outcome}</Text>
      </View>
      {diff ? (
        <View style={styles.body}>
          {diff.hunks.map((hunk, index) => {
            const shown = hunk.slice(0, remaining);
            remaining -= shown.length;
            if (!shown.length) return null;
            const file = diff.paths?.[index];
            const separator = index > 0 && file && file !== diff.paths?.[index - 1] ? basename(file) : '…';
            return <View key={index}>
              {index > 0 ? <Text style={styles.muted}>{separator}</Text> : null}
              {shown.map((line, i) => {
                const color = line.op === 'add' ? theme.tokens.status.success : line.op === 'del' ? theme.tokens.status.error : theme.tokens.text.muted;
                return <Text key={i} selectable style={[styles.mono, line.op !== 'ctx' && { backgroundColor: tint(color, 0.14) }]}>
                  <Text style={{ color }}>{`${line.newNum ?? line.oldNum ?? ''} ${line.op === 'add' ? '+' : line.op === 'del' ? '-' : ' '} `}</Text>
                  <MarkedLine line={line} color={color} />
                </Text>;
              })}
            </View>;
          })}
          {!open && total > limit ? <Text style={styles.muted}>{m.term_mais_linhas({ n: total - limit })}</Text> : null}
          {open && total > MAX_LINES ? <>
            <Text style={styles.muted}>{m.native_side_diff_truncated()}</Text>
            {onOpen ? <Pressable onPress={() => onOpen(use)} style={styles.control} accessibilityRole="button"><Text style={styles.muted}>{m.tool_ver_separado()}</Text></Pressable> : null}
          </> : null}
          {open && expandable ? <Pressable onPress={toggle} style={styles.control} accessibilityRole="button"><Text style={styles.muted}>{m.term_recolher()}</Text></Pressable> : null}
        </View>
      ) : open ? (
        <Pressable style={styles.body} accessible={false} {...bodyPress}>
          <Text style={styles.muted}>{m.native_tool_input()}</Text>
          <Text style={styles.mono} selectable>{JSON.stringify(use.tool_input ?? {}, null, 2)}</Text>
          <Text style={styles.muted}>{m.tool_saida()}</Text>
          <ExitCodeLine output={result?.result} />
          <Text style={[styles.mono, failed && { color: theme.tokens.status.error }]} selectable>{result?.result || outcome}</Text>
        </Pressable>
      ) : null}
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  header: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[2], minHeight: 22 },
  dot: { width: 7, height: 7, borderRadius: theme.base.radius.xs, marginTop: theme.base.space[1.5] },
  mono: { flexShrink: 1, fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, lineHeight: theme.base.text.xs * 1.5, color: theme.tokens.text.primary },
  name: { fontWeight: theme.base.weight.bold },
  muted: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  outcome: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[2], paddingLeft: theme.base.space[3] },
  body: { paddingLeft: 38, paddingTop: theme.base.space[1] },
  control: { minHeight: 44, justifyContent: 'center' },
}));
