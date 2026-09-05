import { Pressable, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { summarizeToolInput, toolPhase, type ChatEvent } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { toolIcon } from './toolIcon';
import * as m from '../../paraglide/messages';

// ChatEvent.ts é epoch em SEGUNDOS (backend/app/transcript.py:_ts).
function duracao(use: ChatEvent, result?: ChatEvent | null): string | null {
  if (!use.ts || !result?.ts) return null;
  const s = Math.max(0, result.ts - use.ts);
  return s < 60 ? `${s.toFixed(s < 10 ? 1 : 0)}s` : `${Math.floor(s / 60)}m${Math.round(s % 60)}s`;
}

// Card curto: ícone do verbo + 1 linha + duração/estado. Toque abre o detalhe.
// `semNome`: dentro de um grupo do mesmo tipo o nome já está no cabeçalho.
export function ToolCard({ use, result, onPress, semNome }: { use: ChatEvent; result?: ChatEvent | null; onPress: () => void; semNome?: boolean }) {
  const { theme } = useUnistyles();
  const fase = toolPhase(result ?? null);
  const resumo = summarizeToolInput(use.tool_name, use.tool_input);
  const cor = fase === 'error' ? theme.tokens.status.error : fase === 'pending' ? theme.tokens.accent.base : theme.tokens.text.muted;
  return (
    <Pressable
      onPress={onPress}
      style={({ pressed }) => [styles.card, pressed && { opacity: 0.7 }]}
      accessibilityRole="button"
      accessibilityLabel={`${use.tool_name ?? m.formato_tool_generico()}: ${resumo}`}
    >
      <Icon name={toolIcon(use.tool_name)} size={15} color={cor} />
      {!semNome ? <Text style={[styles.nome, { color: theme.tokens.text.secondary }]}>{use.tool_name}</Text> : null}
      <Text style={[styles.resumo, { color: theme.tokens.text.primary }]} numberOfLines={1}>{resumo}</Text>
      <Text style={[styles.dir, { color: cor }]}>{fase === 'pending' ? '…' : fase === 'error' ? '!' : duracao(use, result) ?? ''}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  // Conteúdo: rgba com o alpha das caixas, nunca vidro com blur.
  card: {
    flexDirection: 'row', alignItems: 'center', gap: 8, paddingHorizontal: 10, paddingVertical: 7,
    borderRadius: theme.base.radius.sm,
    backgroundColor: `rgba(${theme.tokens.glass.rgb.join(',')},${theme.surfaceAlpha * 0.6})`,
  },
  nome: { fontSize: theme.base.text.xs, fontWeight: '600' },
  resumo: { flex: 1, fontSize: theme.base.text.xs, fontFamily: theme.base.fontMono },
  dir: { fontSize: theme.base.text.xxs, fontFamily: theme.base.fontMono, minWidth: 28, textAlign: 'right' },
}));
