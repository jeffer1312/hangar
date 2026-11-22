import { memo, useMemo, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { splitTerminalRun, summarizeToolResult, terminalFoldTitle, toolPhase, type ChatEvent } from '@hangar/core';
import { TerminalTool } from './TerminalTool';
import { ThinkingBlock } from '../ThinkingBlock';
import * as m from '../../paraglide/messages';

const NO_PLUGINS: readonly string[] = [];
const SHOWN_CALLS = 40;
type Props = { parts: ChatEvent[]; resultOf: (tool: ChatEvent) => ChatEvent | null; onExpand?: () => void; onOpen?: (tool: ChatEvent) => void; pluginServers?: readonly string[] };

function Outcome({ text, failed = false }: { text: string; failed?: boolean }) {
  const { theme } = useUnistyles();
  return <View style={styles.outcome}>
    <Text style={styles.muted}>⎿</Text>
    <Text style={[styles.muted, { flex: 1 }, failed && { color: theme.tokens.status.error }]} numberOfLines={1}>{text}</Text>
  </View>;
}

const TerminalFold = memo(function TerminalFold({ parts, resultOf, onExpand, onOpen }: Props) {
  const { theme } = useUnistyles();
  const [open, setOpen] = useState(false);
  const [shown, setShown] = useState(SHOWN_CALLS);
  const live = parts.findLast((tool) => toolPhase(resultOf(tool)) === 'pending');
  const errors = parts.filter((tool) => toolPhase(resultOf(tool)) === 'error');
  const title = terminalFoldTitle(parts, !!live);
  return <View style={styles.group}>
    <Pressable style={styles.header} accessibilityRole="button" accessibilityLabel={title} accessibilityState={{ expanded: open }}
      onPress={() => { if (!open) onExpand?.(); setOpen((value) => !value); }}>
      <View style={[styles.dot, live && { backgroundColor: theme.tokens.accent.base }]} />
      <Text style={[styles.muted, { flex: 1 }]}>{title}</Text>
    </Pressable>
    {/* Fechada, só as falhas: a linha do comando em voo surgia e sumia a cada comando e fazia a conversa pular. */}
    {!open ? <>
      {errors.map((tool) => <Outcome key={tool.id} text={summarizeToolResult(resultOf(tool), tool.tool_name)} failed />)}
    </> : <>
      {parts.slice(0, shown).map((tool) => <TerminalTool key={tool.id} use={tool} result={resultOf(tool)} onExpand={onExpand} onOpen={onOpen} />)}
      {shown < parts.length ? <Pressable style={styles.header} accessibilityRole="button" onPress={() => { onExpand?.(); setShown((n) => n + SHOWN_CALLS); }}>
        <Text style={styles.muted}>{m.tool_mostrar_mais_linhas({ n: parts.length - shown })}</Text>
      </Pressable> : null}
    </>}
  </View>;
});

/** Só dobra sequências de ferramentas compatíveis; pensamento e edição ficam expostos. */
export const TerminalToolGroup = memo(function TerminalToolGroup({ parts, resultOf, onExpand, onOpen, pluginServers = NO_PLUGINS }: Props) {
  const blocks = useMemo(() => {
    const out: Array<{ kind: 'thinking'; tool: ChatEvent } | ReturnType<typeof splitTerminalRun<ChatEvent>>[number]> = [];
    let run: ChatEvent[] = [];
    // O grupo do pensamento mantém as chamadas expostas, como render_terminal_thinking do nativo.
    const thoughtGroup = parts[0]?.kind === 'thinking';
    const flush = () => {
      out.push(...(thoughtGroup ? run.map((tool) => ({ kind: 'tool' as const, tool })) : splitTerminalRun(run, pluginServers)));
      run = [];
    };
    for (const part of parts) {
      if (part.tool_name === 'ToolSearch') continue;
      if (part.kind === 'thinking') { flush(); out.push({ kind: 'thinking', tool: part }); }
      else run.push(part);
    }
    flush();
    return out;
  }, [parts, pluginServers]);
  return <View style={styles.group}>{blocks.map((block) => block.kind === 'fold'
    ? <TerminalFold key={block.tools[0].id} parts={block.tools} resultOf={resultOf} onExpand={onExpand} onOpen={onOpen} />
    : block.kind === 'thinking' ? <ThinkingBlock key={block.tool.id} ev={block.tool} terminal />
      : <TerminalTool key={block.tool.id} use={block.tool} result={resultOf(block.tool)} onExpand={onExpand} onOpen={onOpen} />)}</View>;
});

const styles = StyleSheet.create((theme) => ({
  group: { gap: theme.base.space[2] },
  header: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 44 },
  dot: { width: 7, height: 7, borderRadius: theme.base.radius.xs },
  muted: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  outcome: { flexDirection: 'row', gap: theme.base.space[2], paddingLeft: theme.base.space[4] },
}));
