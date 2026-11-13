import { memo } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { nomeFerramenta, summarizeToolInput, summarizeToolResult, toolPhase, toolVerbo, type ChatEvent } from '@hangar/core';
import { callTarget } from './fold';
import type { Ferramentas } from '../../stores/aparencia';
import { Icon } from '../../ui/Icon';
import { TerminalTool } from './TerminalTool';
import { DesktopToolCall } from './DesktopToolGroup';
import * as m from '../../paraglide/messages';

// Uma chamada como linha, no desenho escolhido em Aparência › Chamadas de ferramenta:
// - Árvore: verbo apagado + alvo em mono. Rodando, o nome cru em destaque com "· em execução";
//   falhou, a linha inteira na cor de aviso com a primeira linha do erro.
// - Clássico (render_tool do Rust): nome em negrito, o resumo inteiro apagado e o estado à direita.
// - Chips (chip_button do Rust): verbo numa coluna, alvo em mono e o desfecho com ✓ ou ✕ à direita.
// Toque abre o detalhe. `onPress` recebe o próprio evento: a prop é a MESMA referência em toda a lista
// e o `memo` daqui não é anulado por uma arrow nova a cada token do streaming.
export const ToolCard = memo(function ToolCard({
  use,
  result,
  onPress,
  look = 'tree',
  escrevendo = false,
  onExpand,
}: {
  use: ChatEvent;
  result?: ChatEvent | null;
  onPress: (use: ChatEvent) => void;
  look?: Ferramentas;
  /** Pedido da chamada ainda sendo escrito pelo modelo (SSE `ferramenta`): não roda ainda. */
  escrevendo?: boolean;
  onExpand?: () => void;
}) {
  const { theme } = useUnistyles();
  const fase = toolPhase(result ?? null);
  const resumo = summarizeToolInput(use.tool_name, use.tool_input);
  const alvo = callTarget(use.tool_name, resumo);
  const vivo = fase === 'pending';
  const falhou = fase === 'error';
  const cor = falhou ? theme.tokens.status.warning : theme.tokens.text.muted;
  const erro = falhou ? summarizeToolResult(result, use.tool_name) : '';
  const rodando = escrevendo ? m.tool_fase_escrevendo() : m.estado_em_execucao();

  // Claude Desktop: chamada solta vira a mesma linha de resumo; o Agent fica na linha da Árvore.
  if (look === 'claudeDesktop' && use.tool_name !== 'Agent') return <DesktopToolCall use={use} result={result ?? null} onExpand={onExpand} />;

  if (look === 'terminal') return <TerminalTool use={use} result={result} escrevendo={escrevendo} onExpand={onExpand} onOpen={onPress} />;

  if (look === 'chips') {
    const desfecho = escrevendo ? m.tool_fase_escrevendo() : vivo ? m.native_chip_running() : summarizeToolResult(result, use.tool_name);
    const verbo = toolVerbo(use.tool_name);
    return (
      <Pressable
        onPress={() => onPress(use)}
        style={({ pressed }) => [styles.chip, pressed && { backgroundColor: theme.tokens.bg.hover }]}
        accessibilityRole="button"
        accessibilityLabel={[verbo, alvo, desfecho].filter(Boolean).join(', ')}
      >
        <Text style={[styles.chipVerbo, { color: theme.tokens.text.secondary }]} numberOfLines={1}>{verbo}</Text>
        <Text style={[styles.alvo, styles.chipAlvo, { color: theme.tokens.text.primary }]} numberOfLines={1}>{alvo}</Text>
        <View style={styles.fimLinha}>
          {!vivo ? (
            <Icon name={falhou ? 'CircleX' : 'Check'} size={13} color={falhou ? theme.tokens.status.warning : theme.tokens.status.success} />
          ) : null}
          <Text
            style={[styles.fim, { color: vivo ? theme.tokens.accent.base : falhou ? theme.tokens.status.warning : theme.tokens.text.secondary }]}
            numberOfLines={1}
          >
            {desfecho}
          </Text>
        </View>
      </Pressable>
    );
  }

  const classico = look === 'classic';
  const rotulo = classico || vivo ? nomeFerramenta(use.tool_name) : toolVerbo(use.tool_name);
  const fim = vivo ? `· ${rodando}` : erro && erro !== alvo ? `· ${erro}` : '';
  return (
    <Pressable
      onPress={() => onPress(use)}
      style={({ pressed }) => [styles.line, pressed && { backgroundColor: theme.tokens.bg.hover }]}
      accessibilityRole="button"
      accessibilityLabel={[rotulo, alvo, vivo ? rodando : falhou ? erro || m.formato_tool_falhou() : ''].filter(Boolean).join(', ')}
    >
      <Text
        style={[
          styles.verbo,
          { color: vivo || classico ? theme.tokens.text.primary : falhou ? cor : theme.tokens.text.secondary },
          (vivo || classico) && styles.vivo,
          classico && falhou && { color: cor },
        ]}
        numberOfLines={1}
      >
        {rotulo}
      </Text>
      <Text style={[styles.alvo, { color: cor }, classico && styles.semMono]} numberOfLines={1}>{classico ? resumo : alvo}</Text>
      {fim ? <Text style={[styles.fim, { color: vivo && classico ? theme.tokens.accent.base : cor }]} numberOfLines={1}>{fim}</Text> : null}
    </Pressable>
  );
});

const styles = StyleSheet.create((theme) => ({
  // Sem fundo e sem borda: a linha é texto apagado sobre o papel de parede, como no nativo.
  line: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    minHeight: 36,
    paddingHorizontal: theme.base.space[1],
    borderRadius: theme.base.radius.sm,
  },
  verbo: { flexShrink: 0, maxWidth: '40%', fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
  vivo: { fontWeight: theme.base.weight.semibold },
  alvo: { flexShrink: 1, minWidth: 0, fontSize: theme.base.text.xs, fontFamily: theme.base.fontMono },
  semMono: { fontFamily: undefined },
  fim: { flexShrink: 0, maxWidth: '45%', fontSize: theme.base.text.xs },
  // Linha da tabela dos Chips: quem desenha a caixa e as divisórias é o grupo.
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    minHeight: 40,
    paddingHorizontal: theme.base.space[3],
  },
  chipVerbo: { minWidth: 56, flexShrink: 0, fontSize: theme.base.text.xs },
  chipAlvo: { flex: 1 },
  fimLinha: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], flexShrink: 0, maxWidth: '45%' },
}));
