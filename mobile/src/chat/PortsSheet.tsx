// Portas que os processos da sessão abriram; tocar abre a prévia no navegador do sistema.
import { useState, type ReactNode } from 'react';
import { ActivityIndicator, Linking, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useShallow } from 'zustand/react/shallow';
import { openPortPreview, PortNotPageError, type PortEntry } from '@hangar/core';
import { Sheet } from '../ui/Sheet';
import { Icon } from '../ui/Icon';
import { haptic } from '../ui/haptics';
import { useServers } from '../stores/servers';
import { useSessions } from '../stores/sessions';
import * as m from '../paraglide/messages';

const reason = (e: unknown) => (e instanceof Error ? e.message.replace(/^\d{3}: /, '') : String(e));

/** Uma porta da folha; também desenha a animação da novidade. `busy` é a porta que está abrindo. */
export function PortRow({ port: p, busy, failure, onPress }: { port: PortEntry; busy: number | null; failure: string | null; onPress: () => void }) {
  const { theme } = useUnistyles();
  const label = p.process ? `${p.port} · ${p.process}` : String(p.port);
  const opening = busy === p.port;
  return (
    <View>
      <Pressable
        onPress={onPress}
        disabled={busy !== null}
        style={[styles.row, busy !== null && !opening ? { opacity: 0.5 } : null]}
        accessibilityRole="button"
        accessibilityLabel={m.mobile_ports_open_label({ label })}
        accessibilityState={{ busy: opening, disabled: busy !== null }}
      >
        <Text style={[styles.port, { color: theme.tokens.text.primary }]} numberOfLines={1}>{label}</Text>
        {opening ? <ActivityIndicator color={theme.tokens.text.muted} /> : <Icon name="ExternalLink" size={16} color={theme.tokens.text.muted} />}
      </Pressable>
      {failure ? (
        <Text style={[styles.error, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{failure}</Text>
      ) : null}
    </View>
  );
}

export function PortsSheet({ open, onClose, serverId, name }: { open: boolean; onClose: () => void; serverId: string; name: string }) {
  const { theme } = useUnistyles();
  const server = useServers((s) => s.servers.find((x) => x.id === serverId));
  const list = useSessions(useShallow((s) => {
    const bucket = s.byServer.find((b) => b.server.id === serverId);
    const row = s.rows.find((r) => r.serverId === serverId && r.name === name);
    return { found: !!row, ports: row?.ports, error: bucket?.error ?? null, loaded: !!bucket?.loaded, label: bucket?.server.label ?? '' };
  }));
  const [busy, setBusy] = useState<number | null>(null);
  const [failure, setFailure] = useState<{ port: number; text: string } | null>(null);

  const openPort = async (p: PortEntry) => {
    if (!server || busy !== null) return;
    setBusy(p.port);
    setFailure(null);
    try {
      const { url } = await openPortPreview(server, p.port);
      await Linking.openURL(url);
    } catch (e) {
      haptic.error();
      setFailure({ port: p.port, text: e instanceof PortNotPageError ? m.mobile_ports_not_page() : reason(e) });
    } finally {
      setBusy(null);
    }
  };

  const listError = list.error === 'offline' ? m.lista_servidor_offline({ label: list.label }) : `${list.label}: ${list.error}`;
  let body: ReactNode;
  if (!list.found && list.error) {
    body = <PortsStatus text={listError} tone="warning" />;
  } else if (!list.found && !list.loaded) {
    body = (
      <View style={styles.loading}>
        <ActivityIndicator color={theme.tokens.text.muted} />
        <Text style={[styles.status, { color: theme.tokens.text.muted }]}>{m.mobile_ports_loading()}</Text>
      </View>
    );
  } else if (!list.ports?.length) {
    body = <PortsStatus text={m.mobile_ports_empty()} />;
  } else {
    body = list.ports.map((p) => (
      <PortRow key={p.port} port={p} busy={busy} failure={failure?.port === p.port ? failure.text : null} onPress={() => void openPort(p)} />
    ));
  }

  return (
    <Sheet open={open} sizes={['auto']} onDismiss={onClose}>
      <PortsBody>{body}</PortsBody>
    </Sheet>
  );
}

/** Título, conteúdo e o aviso de uma prévia por vez; também desenha a animação da novidade. */
export function PortsBody({ children }: { children: ReactNode }) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.inner}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.mobile_ports_title()}</Text>
      {children}
      <Text style={[styles.note, { color: theme.tokens.text.muted }]}>{m.mobile_ports_one_preview()}</Text>
    </View>
  );
}

/** Texto de estado da folha (vazio, carregando, erro). */
export function PortsStatus({ text, tone }: { text: string; tone?: 'warning' }) {
  const { theme } = useUnistyles();
  return <Text style={[styles.status, { color: tone ? theme.tokens.status.warning : theme.tokens.text.muted }]}
    accessibilityRole={tone ? 'alert' : undefined}>{text}</Text>;
}

const styles = StyleSheet.create((theme) => ({
  inner: { gap: theme.base.space[1], padding: theme.base.space[3] },
  title: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold, marginBottom: theme.base.space[2] },
  loading: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 44 },
  status: { fontSize: theme.base.text.sm, paddingVertical: theme.base.space[2] },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    minHeight: 52,
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.md,
  },
  port: { flex: 1, fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  error: { fontSize: theme.base.text.sm, paddingHorizontal: theme.base.space[2], paddingBottom: theme.base.space[2] },
  note: { fontSize: theme.base.text.xs, marginTop: theme.base.space[2] },
}));
