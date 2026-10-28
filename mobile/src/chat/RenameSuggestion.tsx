import { memo, useCallback, useEffect, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { dismissRenameSuggestion, renameSession, requestRenameSuggestionOnce } from '@hangar/core';
import * as m from '../paraglide/messages';
import { useServers } from '../stores/servers';
import { useSessions } from '../stores/sessions';
import { toast } from '../ui/Toast';

const messageOf = (e: unknown): string => (e instanceof Error ? e.message : String(e));

// Faixa "Renomear para X?" da sessão de nome genérico. Pede a sugestão uma vez ao abrir a conversa;
// falha ao gerar não aparece, só falha de ação da pessoa.
export const RenameSuggestion = memo(function RenameSuggestion({ serverId, name, hasUserMessage }: {
  serverId: string;
  name: string;
  hasUserMessage: boolean;
}) {
  const router = useRouter();
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  const rowLoaded = useSessions((s) => s.rows.some((r) => r.serverId === serverId && r.name === name));
  const rowSuggestion = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.rename_suggestion ?? null);
  const [returned, setReturned] = useState<string | null>(null);
  const [hidden, setHidden] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const row = useSessions.getState().rows.find((r) => r.serverId === serverId && r.name === name);
    if (!server || !row) return;
    requestRenameSuggestionOnce(server, row, hasUserMessage)?.then((s) => { if (s) setReturned(s); });
  }, [server, serverId, name, rowLoaded, rowSuggestion, hasUserMessage]);

  const suggestion = hidden ? null : rowSuggestion || returned;

  const apply = useCallback(() => {
    if (!server || !suggestion) return;
    setBusy(true);
    renameSession(name, suggestion, server)
      .then((r) => {
        toast.ok(r.name);
        // `replace`, como a troca de sessão: a conversa de nome antigo não fica na pilha.
        if (r.name !== name) router.replace(`/s/${serverId}/${r.name}` as never);
      })
      .catch((e: unknown) => toast.erro(messageOf(e)))
      .finally(() => setBusy(false));
  }, [server, suggestion, name, serverId, router]);

  const dismiss = useCallback(() => {
    if (!server) return;
    setBusy(true);
    dismissRenameSuggestion(server, name)
      .then(() => setHidden(true))
      .catch((e: unknown) => toast.erro(messageOf(e)))
      .finally(() => setBusy(false));
  }, [server, name]);

  if (!suggestion || !server) return null;
  const title = m.rename_suggest_title({ name: suggestion });
  return (
    <View style={styles.wrap} accessibilityLabel={title}>
      <Text style={styles.title} numberOfLines={2}>{title}</Text>
      <View style={styles.actions}>
        <Pressable onPress={dismiss} disabled={busy} accessibilityRole="button" accessibilityState={{ disabled: busy }}
                   style={({ pressed }) => [styles.btn, styles.secondary, pressed && styles.pressed, busy && styles.off]}>
          <Text style={styles.secondaryText}>{m.rename_suggest_dismiss()}</Text>
        </Pressable>
        <Pressable onPress={apply} disabled={busy} accessibilityRole="button" accessibilityState={{ disabled: busy, busy }}
                   style={({ pressed }) => [styles.btn, styles.primary, pressed && styles.primaryPressed, busy && styles.off]}>
          <Text style={styles.primaryText}>{m.rename_suggest_apply()}</Text>
        </Pressable>
      </View>
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  wrap: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: theme.base.space[2],
    marginHorizontal: theme.base.space[3],
    marginBottom: theme.base.space[2],
    paddingVertical: theme.base.space[2],
    paddingHorizontal: theme.base.space[3],
    borderWidth: 1,
    borderLeftWidth: 3,
    borderColor: theme.tokens.border.default,
    borderLeftColor: theme.tokens.accent.base,
    borderRadius: theme.base.radius.md,
  },
  title: {
    flexGrow: 1,
    flexShrink: 1,
    minWidth: 160,
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
    color: theme.tokens.text.primary,
  },
  actions: { flexDirection: 'row', gap: theme.base.space[2], marginLeft: 'auto' },
  btn: {
    minHeight: 44,
    paddingHorizontal: theme.base.space[3],
    borderWidth: 1,
    borderRadius: theme.base.radius.lg,
    alignItems: 'center',
    justifyContent: 'center',
  },
  secondary: { borderColor: theme.tokens.border.default },
  secondaryText: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  pressed: { backgroundColor: theme.tokens.bg.hover },
  primary: { backgroundColor: theme.tokens.accent.base, borderColor: theme.tokens.accent.base },
  primaryPressed: { backgroundColor: theme.tokens.accent.press, borderColor: theme.tokens.accent.press },
  primaryText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.ink.onAccent },
  off: { opacity: 0.45 },
}));
