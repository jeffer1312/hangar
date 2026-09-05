import { useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { getCommands } from '@hangar/core';
import type { CommandInfo } from '@hangar/core';
import { Sheet } from '../ui/Sheet';
import * as m from '../paraglide/messages';

// Lista de `/comandos` da sessão, filtrada pelo que já foi digitado depois da barra. Escolher
// devolve o `display` pro composer; quem digita o argumento é a pessoa.
export function CommandSheet({
  open,
  onClose,
  name,
  filtro,
  onEscolher,
}: {
  open: boolean;
  onClose: () => void;
  name: string;
  filtro: string;
  onEscolher: (display: string) => void;
}) {
  const { theme } = useUnistyles();
  const [todos, setTodos] = useState<CommandInfo[] | null>(null);
  const [erro, setErro] = useState('');

  useEffect(() => {
    if (!open || todos) return;
    let vivo = true;
    getCommands(name)
      .then((c) => {
        if (vivo) setTodos(c);
      })
      .catch((e: unknown) => {
        if (vivo) setErro(e instanceof Error ? e.message : String(e));
      });
    return () => {
      vivo = false;
    };
  }, [open, name, todos]);

  const q = filtro.toLowerCase();
  const lista = (todos ?? []).filter((c) => c.name.toLowerCase().includes(q) || c.display.toLowerCase().includes(q));

  return (
    <Sheet open={open} sizes={['medium']} onDismiss={onClose} scrollable>
      <ScrollView contentContainerStyle={styles.inner}>
        <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.comandos_titulo()}</Text>
        {erro ? (
          <Text style={[styles.muted, { color: theme.tokens.status.error }]}>{erro}</Text>
        ) : !todos ? (
          <ActivityIndicator color={theme.tokens.text.secondary} />
        ) : lista.length === 0 ? (
          <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comandos_vazio()}</Text>
        ) : (
          lista.map((c) => (
            <Pressable
              key={`${c.source}:${c.name}`}
              onPress={() => onEscolher(c.display)}
              style={styles.item}
              accessibilityRole="button"
              accessibilityLabel={c.display}
            >
              <View style={styles.txt}>
                <Text style={[styles.nome, { color: theme.tokens.text.primary }]} numberOfLines={1}>
                  {c.display}
                  {c.argumentHint ? <Text style={{ color: theme.tokens.text.muted }}> {c.argumentHint}</Text> : null}
                </Text>
                {c.description ? (
                  <Text style={[styles.desc, { color: theme.tokens.text.muted }]} numberOfLines={1}>
                    {c.description}
                  </Text>
                ) : null}
              </View>
            </Pressable>
          ))
        )}
      </ScrollView>
    </Sheet>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: { gap: theme.base.space[1], padding: theme.base.space[3] },
  title: { fontSize: theme.base.text.base, fontWeight: '600', marginBottom: theme.base.space[2] },
  muted: { fontSize: theme.base.text.sm, textAlign: 'center', paddingVertical: theme.base.space[3] },
  item: {
    minHeight: 48,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.md,
  },
  txt: { gap: 1 },
  nome: { fontSize: theme.base.text.base, fontFamily: theme.base.fontMono },
  desc: { fontSize: theme.base.text.xs },
}));
