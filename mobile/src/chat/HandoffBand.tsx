import { memo, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { cancelHandoffForServer, handoffTitle, modelDisplay } from '@hangar/core';
import * as m from '../paraglide/messages';
import { useServers } from '../stores/servers';
import { useSessions } from '../stores/sessions';
import { Icon } from '../ui/Icon';
import { ProviderGlyph } from '../ui/ProviderGlyph';
import { toast } from '../ui/Toast';

const messageOf = (e: unknown): string => (e instanceof Error ? e.message : String(e));

// Faixa da troca de agente pedida pelo seletor de modelos (no app de PC): esperando o turno acabar, em curso ou
// que falhou. O que for enviado enquanto ela espera vai ao modelo novo.
export const HandoffBand = memo(function HandoffBand({ serverId, name }: { serverId: string; name: string }) {
  const { theme } = useUnistyles();
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  const row = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name) ?? null);
  const [busy, setBusy] = useState(false);
  const handoff = row?.handoff;
  if (!row || !handoff) return null;
  const from = modelDisplay(row.provider, row.model);
  const to = modelDisplay(handoff.provider, handoff.model);
  const title = handoffTitle(handoff, row.state);
  const cancel = () => {
    if (!server) return;
    setBusy(true);
    cancelHandoffForServer(server, name)
      .catch((e) => toast.erro(messageOf(e)))
      .finally(() => setBusy(false));
  };
  return (
    <View style={styles.wrap} accessibilityLabel={`${title}: ${from} → ${to}`} accessibilityLiveRegion="polite">
      <View style={styles.head}>
        <Text style={[styles.title, handoff.state === 'failed' && styles.failed]} numberOfLines={2}>{title}</Text>
        <View style={styles.route}>
          <ProviderGlyph provider={row.provider ?? ''} size={14} />
          <Text style={styles.model} numberOfLines={1}>{from}</Text>
          <Icon name="ArrowRight" size={12} color={theme.tokens.text.muted} />
          <ProviderGlyph provider={handoff.provider} size={14} />
          <Text style={styles.model} numberOfLines={1}>{to}</Text>
        </View>
      </View>
      {handoff.state === 'waiting' && (
        <Pressable onPress={cancel} disabled={busy} accessibilityRole="button" accessibilityState={{ disabled: busy, busy }}
                   style={({ pressed }) => [styles.btn, pressed && styles.pressed, busy && styles.off]}>
          <Text style={styles.btnText}>{m.handoff_cancel()}</Text>
        </Pressable>
      )}
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  wrap: {
    flexDirection: 'row',
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
  head: { flex: 1, gap: theme.base.space[1] },
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  failed: { color: theme.tokens.status.warning },
  route: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  model: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted, flexShrink: 1 },
  btn: {
    minHeight: 44,
    paddingHorizontal: theme.base.space[3],
    borderWidth: 1,
    borderRadius: theme.base.radius.lg,
    borderColor: theme.tokens.border.default,
    alignItems: 'center',
    justifyContent: 'center',
  },
  btnText: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  pressed: { backgroundColor: theme.tokens.bg.hover },
  off: { opacity: 0.45 },
}));
