import { useEffect } from 'react';
import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { sortSessions } from '@hangar/core';
import type { AggSession } from '@hangar/core';
import { Sheet } from '../ui/Sheet';
import { StateDot } from '../ui/StateDot';
import { useSessions } from '../stores/sessions';
import * as m from '../paraglide/messages';

// Trocar de sessão sem voltar pra lista. `replace` e não `push`: empilhar chats faria o voltar
// percorrer a fila de sessões visitadas em vez de sair do chat.
export function SessionPickerSheet({
  open,
  onClose,
  atual,
}: {
  open: boolean;
  onClose: () => void;
  atual: string;
}) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const rows = useSessions((s) => s.rows);
  useEffect(() => {
    if (!open) return;
    const release = useSessions.getState().retain();
    return () => release();
  }, [open]);
  const ordenadas = sortSessions(rows) as AggSession[];

  return (
    <Sheet open={open} sizes={['medium', 'large']} onDismiss={onClose} scrollable>
      <ScrollView contentContainerStyle={styles.inner}>
        <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.sessao_trocar_de()}</Text>
        {ordenadas.map((s) => (
          <Pressable
            key={`${s.serverId}::${s.name}`}
            onPress={() => {
              onClose();
              router.replace(`/s/${s.serverId}/${s.name}` as never);
            }}
            style={styles.item}
            accessibilityRole="button"
            accessibilityLabel={s.name}
            accessibilityState={{ selected: s.name === atual }}
          >
            <StateDot state={s.state} />
            <Text
              style={[styles.nome, { color: theme.tokens.text.primary, fontWeight: s.name === atual ? '700' : '500' }]}
              numberOfLines={1}
            >
              {s.name}
            </Text>
            <Text style={[styles.servidor, { color: theme.tokens.text.muted }]} numberOfLines={1}>
              {s.serverLabel}
            </Text>
          </Pressable>
        ))}
      </ScrollView>
    </Sheet>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: { gap: theme.base.space[1], padding: theme.base.space[3] },
  title: { fontSize: theme.base.text.base, fontWeight: '600', marginBottom: theme.base.space[2] },
  item: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    minHeight: 48,
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.md,
  },
  nome: { flex: 1, fontSize: theme.base.text.base },
  servidor: { fontSize: theme.base.text.xs, maxWidth: 120 },
}));
