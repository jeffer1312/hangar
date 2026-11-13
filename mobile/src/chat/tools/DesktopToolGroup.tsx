import { memo, useCallback, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { nomeFerramenta, summarizeToolInput, toolPhase, type ChatEvent } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { callTarget } from './fold';
import { desktopFamily, desktopSummary } from './desktopLook';
import { ExitCodeLine } from './ExitCodeLine';
import { headLines } from './TerminalTool';
import * as m from '../../paraglide/messages';
import { useSelectableToggle } from './useSelectableToggle';

const SHOWN_CALLS = 40;
const VERB: Record<ReturnType<typeof desktopFamily>, () => string> = {
  rodou: m.ferr_desktop_linha_rodou, leu: m.ferr_desktop_linha_leu, editou: m.ferr_desktop_linha_editou,
  criou: m.ferr_desktop_linha_criou, buscou: m.ferr_desktop_linha_buscou, web: m.ferr_desktop_linha_web,
  ferramentas: m.ferr_desktop_linha_ferramentas, outra: m.ferr_desktop_linha_outra,
};

type Props = {
  parts: ChatEvent[];
  resultOf: (tool: ChatEvent) => ChatEvent | null;
  onExpand?: () => void;
  /** Abre o detalhe da chamada: é lá que a saída de um Bash ainda rodando é lida ao vivo. */
  onOpen?: (tool: ChatEvent) => void;
  /** Pedido ainda sendo escrito pelo modelo: o resumo diz escrevendo, não em execução. */
  writing?: boolean;
  /** Pensamento e Agent ficam como nos outros estilos: quem desenha é o ToolGroup. */
  renderOther?: (part: ChatEvent) => ReactNode;
};

/** Uma chamada: verbo apagado, alvo, `Falhou` e, aberta, o comando, o código de saída e a saída. */
const DesktopCall = memo(function DesktopCall({ use, result, onExpand, onOpen }: { use: ChatEvent; result: ChatEvent | null; onExpand?: () => void; onOpen?: Props['onOpen'] }) {
  const { theme } = useUnistyles();
  const [open, setOpen] = useState(false);
  const family = desktopFamily(use.tool_name);
  const summary = summarizeToolInput(use.tool_name, use.tool_input);
  const target = family === 'outra' ? [nomeFerramenta(use.tool_name), summary].filter(Boolean).join(' ') : callTarget(use.tool_name, summary);
  const failed = toolPhase(result) === 'error';
  const command = use.tool_input?.['command'];
  const output = useMemo(() => open && result?.result ? headLines(result.result) : null, [open, result?.result]);
  const toggle = () => { if (!open) onExpand?.(); setOpen((value) => !value); };
  const bodyPress = useSelectableToggle(toggle);
  return <View>
    <Pressable onPress={toggle} style={({ pressed }) => [styles.row, pressed && { backgroundColor: theme.tokens.bg.hover }]}
      accessibilityRole="button" accessibilityState={{ expanded: open }}
      accessibilityLabel={[VERB[family](), target, failed ? m.formato_tool_falhou() : ''].filter(Boolean).join(', ')}>
      <Text style={[styles.verb, { color: theme.tokens.text.muted }]} numberOfLines={1}>{VERB[family]()}</Text>
      <Text style={[styles.target, { color: theme.tokens.text.secondary }]} numberOfLines={1}>{target}</Text>
      {failed ? <Text style={[styles.verb, { color: theme.tokens.status.warning }]}>{m.formato_tool_falhou()}</Text> : null}
      <Icon name={open ? 'ChevronDown' : 'ChevronRight'} size={14} color={theme.tokens.text.muted} />
    </Pressable>
    {open ? (
      <Pressable style={styles.body} accessible={false} {...bodyPress}>
        <Text style={[styles.code, { borderColor: theme.tokens.border.subtle, color: theme.tokens.text.primary }]} selectable>
          {typeof command === 'string' ? command : JSON.stringify(use.tool_input ?? {}, null, 2)}
        </Text>
        <ExitCodeLine output={result?.result} />
        {output ? <Text style={[styles.mono, { color: theme.tokens.text.secondary }]} selectable>{output.text}</Text> : null}
        {output?.hidden ? <Text style={[styles.verb, { color: theme.tokens.text.muted }]}>{m.term_mais_linhas({ n: output.hidden })}</Text> : null}
        {onOpen ? <Pressable onPress={() => onOpen(use)} style={styles.open} accessibilityRole="button">
          <Text style={[styles.verb, { color: theme.tokens.text.secondary }]}>{m.tool_ver_separado()}</Text>
        </Pressable> : null}
      </Pressable>
    ) : null}
  </View>;
});

/** Sequência de chamadas: a linha cinza com o resumo e, aberta, o cartão com uma linha por chamada. */
const DesktopRun = memo(function DesktopRun({ calls, resultOf, onExpand, onOpen, writing }: { calls: ChatEvent[]; resultOf: Props['resultOf']; onExpand?: () => void; onOpen?: Props['onOpen']; writing?: boolean }) {
  const [open, setOpen] = useState(false);
  const [shown, setShown] = useState(SHOWN_CALLS);
  return <DesktopRunView calls={calls} resultOf={resultOf} onExpand={onExpand} onOpen={onOpen} writing={writing} open={open} shown={shown}
    onToggle={() => { if (!open) onExpand?.(); setOpen((value) => !value); }}
    onMore={() => { onExpand?.(); setShown((n) => n + SHOWN_CALLS); }} />;
});

/** A sequência desenhada com o estado de fora: a conversa guarda o aberto; a novidade o anima. */
export function DesktopRunView({ calls, resultOf, onExpand, onOpen, writing = false, open, shown = SHOWN_CALLS, onToggle, onMore }: {
  calls: ChatEvent[]; resultOf: Props['resultOf']; onExpand?: () => void; onOpen?: Props['onOpen']; writing?: boolean;
  open: boolean; shown?: number; onToggle?: () => void; onMore?: () => void;
}) {
  const { theme } = useUnistyles();
  const title = useMemo(() => desktopSummary(calls, resultOf, writing), [calls, resultOf, writing]);
  return <View style={styles.run}>
    <Pressable onPress={onToggle}
      style={({ pressed }) => [styles.head, pressed && { backgroundColor: theme.tokens.bg.hover }]}
      accessibilityRole="button" accessibilityLabel={title} accessibilityState={{ expanded: open }}>
      <Text style={[styles.title, { color: theme.tokens.text.muted }]} numberOfLines={2}>{title}</Text>
      <Icon name={open ? 'ChevronUp' : 'ChevronDown'} size={14} color={theme.tokens.text.muted} />
    </Pressable>
    {open ? <View style={[styles.card, { borderColor: theme.tokens.border.subtle }]}>
      {calls.slice(0, shown).map((call, i) => (
        <View key={call.id} style={i > 0 ? { borderTopWidth: 1, borderTopColor: theme.tokens.border.subtle } : undefined}>
          <DesktopCall use={call} result={resultOf(call)} onExpand={onExpand} onOpen={onOpen} />
        </View>
      ))}
      {shown < calls.length ? <Pressable style={styles.row} accessibilityRole="button" onPress={onMore}>
        <Text style={[styles.verb, { color: theme.tokens.text.secondary }]}>{m.tool_mostrar_mais_linhas({ n: calls.length - shown })}</Text>
      </Pressable> : null}
    </View> : null}
  </View>;
}

/** Estilo Claude Desktop: cada sequência de chamadas vira uma linha recolhida; pensamento e Agent cortam. */
export const DesktopToolGroup = memo(function DesktopToolGroup({ parts, resultOf, onExpand, onOpen, writing, renderOther }: Props) {
  // Trecho que cresce não recria as sequências anteriores: cada uma mantém o mesmo array de chamadas.
  const previousRuns = useRef(new Map<string, ChatEvent[]>());
  const { blocks, runs } = useMemo(() => {
    const out: Array<{ run: ChatEvent[] } | { other: ChatEvent }> = [];
    const runs = new Map<string, ChatEvent[]>();
    const close = (run: ChatEvent[]) => {
      if (!run.length) return;
      const before = previousRuns.current.get(run[0].id);
      const kept = before && before.length === run.length && before.every((call, i) => call === run[i]) ? before : run;
      runs.set(run[0].id, kept);
      out.push({ run: kept });
    };
    let run: ChatEvent[] = [];
    for (const part of parts) {
      if (part.kind === 'tool_use' && part.tool_name !== 'Agent') { run.push(part); continue; }
      close(run);
      run = [];
      out.push({ other: part });
    }
    close(run);
    return { blocks: out, runs };
  }, [parts]);
  // Render descartado ou repetido não publica o seu cache.
  useLayoutEffect(() => { previousRuns.current = runs; }, [runs]);
  return <View style={styles.group}>{blocks.map((block) => 'run' in block
    ? <DesktopRun key={block.run[0].id} calls={block.run} resultOf={resultOf} onExpand={onExpand} onOpen={onOpen} writing={writing} />
    : <View key={block.other.id}>{renderOther?.(block.other)}</View>)}</View>;
});

/** Chamada solta fora de grupo: a mesma linha de resumo, com uma chamada só. */
export const DesktopToolCall = memo(function DesktopToolCall({ use, result, onExpand, onOpen, writing }: {
  use: ChatEvent; result: ChatEvent | null; onExpand?: () => void; onOpen?: Props['onOpen']; writing?: boolean;
}) {
  const parts = useMemo(() => [use], [use]);
  const resultOf = useCallback(() => result, [result]);
  return <DesktopToolGroup parts={parts} resultOf={resultOf} onExpand={onExpand} onOpen={onOpen} writing={writing} />;
});

const styles = StyleSheet.create((theme) => ({
  group: { gap: theme.base.space[1] },
  run: { gap: theme.base.space[1] },
  head: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minHeight: 44, alignSelf: 'flex-start', paddingHorizontal: theme.base.space[1], borderRadius: theme.base.radius.sm },
  title: { flexShrink: 1, ...theme.type.meta },
  // Só borda, sem fundo: o cartão segue o papel de parede como a tabela dos Chips.
  card: { borderWidth: 1, borderRadius: theme.base.radius.sm, overflow: 'hidden' },
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 44, paddingHorizontal: theme.base.space[3] },
  verb: { flexShrink: 0, fontSize: theme.base.text.xs },
  target: { flexShrink: 1, minWidth: 0, fontSize: theme.base.text.xs },
  body: { paddingHorizontal: theme.base.space[3], paddingBottom: theme.base.space[3], gap: theme.base.space[2] },
  code: { ...theme.type.mono, borderWidth: 1, borderRadius: theme.base.radius.xs, padding: theme.base.space[2] },
  mono: { ...theme.type.mono },
  open: { minHeight: 44, justifyContent: 'center', alignSelf: 'flex-start' },
}));
