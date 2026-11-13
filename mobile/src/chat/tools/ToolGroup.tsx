import { memo, useCallback, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { ChatEvent } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { ToolCard } from './ToolCard';
import { TerminalToolGroup } from './TerminalToolGroup';
import { DesktopToolGroup } from './DesktopToolGroup';
import { ThinkingBlock } from '../ThinkingBlock';
import { appendFoldSummary, failedLabel, summaryTitle, type FoldSummary } from './fold';
import type { Ferramentas } from '../../stores/aparencia';
import * as m from '../../paraglide/messages';

// Um trecho inteiro é UMA linha da lista virtualizada: abrir 300 linhas de uma vez monta 300 views
// num único item. Mostra as primeiras e deixa o resto atrás de um toque.
const TETO = 40;
// Grupo pequeno nasce aberto nos Chips, como no Rust (CHIPS_OPEN_UP_TO).
const CHIPS_ABERTO_ATE = 5;

// Trecho de trabalho dobrado como no nativo: "› 3 raciocínios · rodou 2 comandos · 1 falhou".
// Aberto, as linhas ficam num fio vertical à esquerda (nos Chips, numa tabela com borda). A chamada
// VIVA aparece sob o cabeçalho mesmo fechado, senão o resumo escondia o que está acontecendo agora.
// Trecho de uma linha só não ganha cabeçalho: dobrar uma linha numa linha não resume nada.
export const ToolGroup = memo(function ToolGroup({
  parts,
  summary,
  resultOf,
  onAbrir,
  onExpand,
  look = 'tree',
  pluginServers,
}: {
  parts: ChatEvent[];
  summary?: FoldSummary;
  resultOf: (t: ChatEvent) => ChatEvent | null;
  onAbrir: (use: ChatEvent) => void;
  onExpand?: () => void;
  look?: Ferramentas;
  pluginServers?: readonly string[];
}) {
  const { theme } = useUnistyles();
  const folded = summary ?? appendFoldSummary(undefined, parts, resultOf);
  const linhas = folded.lines;
  const calls = folded.calls;
  const chips = look === 'chips';
  const [virado, setVirado] = useState(false);
  const aberto = (chips && calls <= CHIPS_ABERTO_ATE) !== virado;
  const [tudo, setTudo] = useState(false);
  const falhas = failedLabel(folded.failures);
  // No Claude Desktop só pensamento e Agent passam por aqui (as chamadas viram a linha de resumo),
  // e os dois se desenham como na Árvore.
  const cardLook: Ferramentas = look === 'claudeDesktop' ? 'tree' : look;
  // Estável entre renders: o DesktopToolGroup a recebe como prop e fica em `memo`.
  const linha = useCallback((p: ChatEvent) =>
    p.kind === 'thinking'
      ? <ThinkingBlock key={p.id} ev={p} />
      : <ToolCard key={p.id} use={p} result={resultOf(p)} onPress={onAbrir} look={cardLook} />, [resultOf, onAbrir, cardLook]);

  if (look === 'terminal') return <TerminalToolGroup parts={parts} resultOf={resultOf} onExpand={onExpand} onOpen={onAbrir} pluginServers={pluginServers} />;
  if (look === 'claudeDesktop') return <DesktopToolGroup parts={parts} resultOf={resultOf} onExpand={onExpand} onOpen={onAbrir} renderOther={linha} />;
  // Tabela dos Chips: caixa com borda e divisória entre as linhas (chip_table do Rust).
  const tabela = (ls: ChatEvent[]) => (
    <View style={[styles.tabela, { borderColor: theme.tokens.border.subtle }]}>
      {ls.map((p, i) => (
        <View key={p.id} style={i > 0 ? { borderTopWidth: 1, borderTopColor: theme.tokens.border.subtle } : undefined}>{linha(p)}</View>
      ))}
    </View>
  );

  if (linhas.length === 0) return null;
  if (linhas.length === 1) return chips && linhas[0].kind === 'tool_use' ? tabela(linhas) : linha(linhas[0]);

  const titulo = summaryTitle(folded);
  const chamadas = chips && calls ? ` · ${calls === 1 ? m.native_chip_calls_1() : m.native_chip_calls({ n: calls })}` : '';
  const visiveis = aberto ? (tudo ? linhas : linhas.slice(0, TETO)) : [];
  return (
    <View style={styles.wrap}>
      <Pressable
        onPress={() => {
          if (!aberto) onExpand?.();
          setVirado((v) => !v);
        }}
        hitSlop={6}
        style={({ pressed }) => [styles.head, pressed && { backgroundColor: theme.tokens.bg.hover }]}
        accessibilityRole="button"
        accessibilityLabel={[titulo + chamadas, falhas].filter(Boolean).join(', ')}
        accessibilityState={{ expanded: aberto }}
      >
        <Icon name={aberto ? 'ChevronDown' : 'ChevronRight'} size={14} color={theme.tokens.text.muted} />
        <Text style={styles.titulo} numberOfLines={1}>
          <Text style={{ color: chips ? theme.tokens.text.primary : theme.tokens.text.muted }}>{titulo}</Text>
          {chamadas ? <Text style={{ color: theme.tokens.text.muted }}>{chamadas}</Text> : null}
          {falhas ? <Text style={{ color: theme.tokens.status.warning }}>{` · ${falhas}`}</Text> : null}
        </Text>
      </Pressable>
      {!aberto && folded.live.size ? (
        <View style={styles.vivas}>{[...folded.live].map(linha)}</View>
      ) : null}
      {aberto ? (
        <View style={chips ? undefined : [styles.fio, { borderLeftColor: theme.tokens.border.default }]}>
          {chips ? tabela(visiveis) : visiveis.map(linha)}
          {!tudo && linhas.length > TETO ? (
            <Pressable onPress={() => { onExpand?.(); setTudo(true); }} style={({ pressed }) => [styles.mais, pressed && { backgroundColor: theme.tokens.bg.hover }]} accessibilityRole="button">
              <Text style={[styles.maisTxt, { color: theme.tokens.text.secondary }]}>{m.tool_mostrar_todas({ n: linhas.length })}</Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  wrap: { gap: theme.base.space[0.5] },
  head: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1.5],
    minHeight: 32,
    paddingHorizontal: theme.base.space[1],
    borderRadius: theme.base.radius.sm,
  },
  titulo: { flex: 1, minWidth: 0, ...theme.type.meta },
  vivas: { paddingLeft: theme.base.space[1] },
  // Fio à esquerda em vez de caixa: o trabalho é aparte da conversa, não um cartão — superfície
  // própria aqui viraria retângulo chapado por cima do papel de parede.
  fio: { marginLeft: theme.base.space[2], paddingLeft: theme.base.space[2], borderLeftWidth: 1 },
  // Os Chips são a exceção pedida: tabela só com borda, sem fundo, para seguir o papel de parede.
  tabela: { borderWidth: 1, borderRadius: theme.base.radius.sm, overflow: 'hidden' },
  mais: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[1] },
  maisTxt: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold },
}));
