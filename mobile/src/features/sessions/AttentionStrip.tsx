import { Pressable, ScrollView } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { attentionFeed, type AggSession } from '@hangar/core';
import { Chip } from '../../ui/Chip';

// "Precisa de você": quem está aguardando resposta, de todos os servidores, mais antigo primeiro.
export function AttentionStrip({ sessions, onOpen }: { sessions: AggSession[]; onOpen: (s: AggSession) => void }) {
  const fila = attentionFeed(sessions);
  if (!fila.length) return null;
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.faixa}>
      {fila.map((s) => (
        <Pressable
          key={`${s.serverId}::${s.name}`}
          onPress={() => onOpen(s)}
          accessibilityRole="button"
          // o label do pai apaga o texto do chip pro leitor de tela — a pergunta vai junto
          accessibilityLabel={s.question ? `${s.name}, ${s.question}` : s.name}
          hitSlop={6}
          style={styles.item}
        >
          <Chip tone="warning" icon="CircleHelp">{s.question ? `${s.name} · ${s.question}` : s.name}</Chip>
        </Pressable>
      ))}
    </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  faixa: { flexDirection: 'row', gap: theme.base.space[2], paddingHorizontal: theme.base.space[3], paddingBottom: theme.base.space[2] },
  item: { maxWidth: 260 },
}));
