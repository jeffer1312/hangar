import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { toolGroupCounts, toolGroupLabel, toolPhase, type ChatEvent } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { ToolCard } from './ToolCard';
import * as m from '../../paraglide/messages';

// Burst de 3+ chamadas = UMA linha ("Ferramentas: 2 rodando • 3 concluídos"). Aberto, lista os
// ToolCards; a chamada VIVA aparece sob o cabeçalho mesmo fechado, senão o painel escondia o agora.
export function ToolGroup({ tools, resultOf, onAbrir }: { tools: ChatEvent[]; resultOf: (t: ChatEvent) => ChatEvent | null; onAbrir: (use: ChatEvent, result: ChatEvent | null) => void }) {
  const { theme } = useUnistyles();
  const [aberto, setAberto] = useState(false);
  const fases = tools.map((t) => toolPhase(resultOf(t)));
  const label = toolGroupLabel(tools.map((t) => t.tool_name));
  const mixed = label === m.lista_ferramentas();
  const erro = fases.includes('error');
  const viva = tools.find((_t, i) => fases[i] === 'pending');
  return (
    <View style={styles.wrap}>
      <Pressable onPress={() => setAberto((v) => !v)} style={styles.head} accessibilityRole="button" accessibilityState={{ expanded: aberto }}>
        <Icon name={aberto ? 'ChevronDown' : 'ChevronRight'} size={14} color={theme.tokens.text.muted} />
        <Text style={[styles.label, { color: erro ? theme.tokens.status.error : theme.tokens.text.secondary }]}>{label}</Text>
        <Text style={[styles.counts, { color: theme.tokens.text.muted }]} numberOfLines={1}>{toolGroupCounts(fases)}</Text>
      </Pressable>
      {!aberto && viva ? <ToolCard use={viva} result={null} semNome={!mixed} onPress={() => onAbrir(viva, null)} /> : null}
      {aberto ? tools.map((t) => <ToolCard key={t.id} use={t} result={resultOf(t)} semNome={!mixed} onPress={() => onAbrir(t, resultOf(t))} />) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: { gap: 4 },
  head: { flexDirection: 'row', alignItems: 'center', gap: 6, paddingVertical: 6, paddingHorizontal: 4 },
  label: { fontSize: theme.base.text.xs, fontWeight: '600' },
  counts: { fontSize: theme.base.text.xxs, flex: 1 },
}));
