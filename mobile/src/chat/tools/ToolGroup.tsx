import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { toolGroupCounts, toolGroupLabel, toolPhase, type ChatEvent } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { ToolCard } from './ToolCard';
import * as m from '../../paraglide/messages';

// Um grupo inteiro é UMA linha da lista virtualizada: abrir 300 cards de uma vez monta 300 views
// num único item. Mostra as primeiras e deixa o resto atrás de um toque.
const TETO = 40;

// Burst de 3+ chamadas = UMA linha ("Ferramentas: 2 rodando • 3 concluídos"). Aberto, lista os
// ToolCards; a chamada VIVA aparece sob o cabeçalho mesmo fechado, senão o painel escondia o agora.
export function ToolGroup({ tools, resultOf, onAbrir }: { tools: ChatEvent[]; resultOf: (t: ChatEvent) => ChatEvent | null; onAbrir: (use: ChatEvent) => void }) {
  const { theme } = useUnistyles();
  const [aberto, setAberto] = useState(false);
  const [tudo, setTudo] = useState(false);
  const fases = tools.map((t) => toolPhase(resultOf(t)));
  const label = toolGroupLabel(tools.map((t) => t.tool_name));
  const mixed = label === m.lista_ferramentas();
  const erro = fases.includes('error');
  const viva = tools.find((_t, i) => fases[i] === 'pending');
  const visiveis = tudo ? tools : tools.slice(0, TETO);
  return (
    <View style={styles.wrap}>
      <Pressable onPress={() => setAberto((v) => !v)} style={styles.head} accessibilityRole="button" accessibilityState={{ expanded: aberto }}>
        <Icon name={aberto ? 'ChevronDown' : 'ChevronRight'} size={14} color={theme.tokens.text.muted} />
        <Text style={[styles.label, { color: erro ? theme.tokens.status.error : theme.tokens.text.secondary }]}>{label}</Text>
        <Text style={[styles.counts, { color: theme.tokens.text.muted }]} numberOfLines={1}>{toolGroupCounts(fases)}</Text>
      </Pressable>
      {!aberto && viva ? <ToolCard use={viva} result={resultOf(viva)} semNome={!mixed} onPress={() => onAbrir(viva)} /> : null}
      {aberto ? visiveis.map((t) => <ToolCard key={t.id} use={t} result={resultOf(t)} semNome={!mixed} onPress={() => onAbrir(t)} />) : null}
      {aberto && !tudo && tools.length > TETO ? (
        <Pressable onPress={() => setTudo(true)} style={styles.mais} accessibilityRole="button">
          <Text style={[styles.maisTxt, { color: theme.tokens.accent.base }]}>{m.tool_mostrar_todas({ n: tools.length })}</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: { gap: 4 },
  head: { flexDirection: 'row', alignItems: 'center', gap: 6, paddingVertical: 6, paddingHorizontal: 4 },
  label: { fontSize: theme.base.text.xs, fontWeight: '600' },
  counts: { fontSize: theme.base.text.xxs, flex: 1 },
  mais: { paddingVertical: 8, paddingHorizontal: 10, minHeight: 44, justifyContent: 'center' },
  maisTxt: { fontSize: theme.base.text.xs, fontWeight: '600' },
}));
