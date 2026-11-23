// Portas que os processos da sessão abriram. Tocar abre a página no app pelo túnel; aparelho sem suporte
// (iOS < 17, WebView sem PROXY_OVERRIDE, binário antigo) abre a prévia no navegador do sistema.
import { useRef, useState, type ReactNode } from 'react';
import { ActivityIndicator, Alert, Linking, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { useShallow } from 'zustand/react/shallow';
import { openPortPreview, PortNotPageError, stopPortServer, type PortEntry } from '@hangar/core';
import { portTunnelSupported } from '../../modules/port-tunnel/support';
import { Sheet } from '../ui/Sheet';
import { Icon } from '../ui/Icon';
import { haptic } from '../ui/haptics';
import { useServers } from '../stores/servers';
import { useSessions } from '../stores/sessions';
import { INK } from '../theme/scale';
import * as m from '../paraglide/messages';

const reason = (e: unknown) => (e instanceof Error ? e.message.replace(/^\d{3}: /, '') : String(e));

function stopReason(e: unknown): string {
  switch ((e as { code?: string }).code) {
    case 'port_stop_not_found': return m.ports_stop_err_not_found();
    case 'port_stop_shared_group': return m.ports_stop_err_shared();
    case 'port_stop_unsupported': return m.ports_stop_err_unsupported();
    default: return m.ports_stop_err_failed({ error: reason(e) });
  }
}

/** Uma porta da folha; também desenha a animação da novidade. `busy` é a porta que está abrindo;
 * `onStop` mostra o "Parar servidor", e `stopping` é a porta que está parando. `onLink` (com o túnel) mostra o
 * "Link de fora" e faz o toque abrir no app. */
export function PortRow({ port: p, busy, failure, onPress, onStop, stopping = null, onLink }: {
  port: PortEntry; busy: number | null; failure: string | null; onPress: () => void; onStop?: () => void; stopping?: number | null;
  onLink?: () => void;
}) {
  const { theme } = useUnistyles();
  const label = p.process ? `${p.port} · ${p.process}` : String(p.port);
  const opening = busy === p.port;
  const inApp = !!onLink;
  return (
    <View>
      <View style={styles.line}>
        <Pressable
          onPress={onPress}
          disabled={busy !== null}
          style={[styles.row, busy !== null && !opening ? { opacity: 0.5 } : null]}
          accessibilityRole="button"
          accessibilityLabel={inApp ? m.mobile_ports_open_app_label({ label }) : m.mobile_ports_open_label({ label })}
          accessibilityState={{ busy: opening, disabled: busy !== null }}
        >
          <Text style={[styles.port, { color: theme.tokens.text.primary }]} numberOfLines={1}>{label}</Text>
          {opening && !inApp ? <ActivityIndicator color={theme.tokens.text.muted} />
            : <Icon name={inApp ? 'ChevronRight' : 'ExternalLink'} size={16} color={theme.tokens.text.muted} />}
        </Pressable>
        {onLink ? (
          <Pressable
            onPress={onLink}
            disabled={busy !== null}
            style={[styles.stop, busy !== null && !opening ? { opacity: 0.5 } : null]}
            accessibilityRole="button"
            accessibilityLabel={m.mobile_ports_link_label({ label })}
            accessibilityState={{ busy: opening, disabled: busy !== null }}
          >
            {opening ? <ActivityIndicator color={theme.tokens.text.muted} /> : <Icon name="Link" size={16} color={theme.tokens.text.muted} />}
          </Pressable>
        ) : null}
        {onStop ? (
          <Pressable
            onPress={onStop}
            disabled={stopping !== null}
            style={[styles.stop, stopping !== null && stopping !== p.port ? { opacity: 0.5 } : null]}
            accessibilityRole="button"
            accessibilityLabel={m.ports_stop_server_label({ label })}
            accessibilityState={{ busy: stopping === p.port, disabled: stopping !== null }}
          >
            {stopping === p.port ? <ActivityIndicator color={theme.tokens.text.muted} /> : <Icon name="Square" size={16} color={theme.tokens.status.warning} />}
          </Pressable>
        ) : null}
      </View>
      {p.shared ? (
        <Text style={[styles.error, { color: theme.tokens.text.muted }]}>{m.mobile_ports_shared()}</Text>
      ) : null}
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
  const [stopping, setStopping] = useState<number | null>(null);
  const router = useRouter();
  // Convite não tem `/api/port-tunnel`: só o dono abre pelo túnel.
  const [tunnel] = useState(() => portTunnelSupported());
  const inApp = tunnel && !!server && !server.invite;
  const afterDismiss = useRef<(() => void) | null>(null);

  const openInApp = (p: PortEntry) => {
    haptic.selection();
    // O iOS precisa concluir o fechamento da folha antes de empilhar outra tela.
    afterDismiss.current = () => router.push(`/s/${encodeURIComponent(serverId)}/${encodeURIComponent(name)}/port?port=${p.port}` as never);
    onClose();
  };

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

  const stopPort = async (p: PortEntry) => {
    if (!server || stopping !== null) return;
    setStopping(p.port);
    setFailure(null);
    try {
      await stopPortServer(name, p.port, server);
      haptic.success();
    } catch (e) {
      haptic.error();
      setFailure({ port: p.port, text: stopReason(e) });
    } finally {
      setStopping(null);
    }
  };

  const confirmStop = (p: PortEntry) => {
    Alert.alert(m.ports_stop_server_title({ port: String(p.port) }), m.ports_stop_server_desc(), [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.ports_stop_server(), style: 'destructive', onPress: () => void stopPort(p) },
    ]);
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
      <PortRow key={p.port} port={p} busy={busy} failure={failure?.port === p.port ? failure.text : null}
        onPress={() => (inApp ? openInApp(p) : void openPort(p))} onLink={inApp ? () => void openPort(p) : undefined}
        onStop={() => confirmStop(p)} stopping={stopping} />
    ));
  }

  return (
    <Sheet open={open} sizes={['auto']} onDismiss={() => {
      const action = afterDismiss.current;
      afterDismiss.current = null;
      onClose();
      action?.();
    }}>
      <PortsBody note={inApp ? m.mobile_ports_tunnel_note() : undefined}>{body}</PortsBody>
    </Sheet>
  );
}

/** Título, conteúdo e o aviso de como as portas abrem; também desenha a animação da novidade. */
export function PortsBody({ children, note }: { children: ReactNode; note?: string }) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.inner}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.mobile_ports_title()}</Text>
      {children}
      <Text style={[styles.note, { color: theme.tokens.text.muted }]}>{note ?? m.mobile_ports_one_preview()}</Text>
    </View>
  );
}

/** Uma aba por porta aberta no app, na tela da página; também desenha a animação da novidade. */
export function PortTabs({ tabs, active, onSelect }: { tabs: number[]; active: number; onSelect: (port: number) => void }) {
  const { theme } = useUnistyles();
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.tabs}
      accessibilityRole="tablist" accessibilityLabel={m.mobile_ports_tunnel_tabs()}>
      {tabs.map((p) => (
        <Pressable key={p} onPress={() => onSelect(p)} style={[styles.tab, p === active && { backgroundColor: theme.tokens.accent.base }]}
          accessibilityRole="tab" accessibilityState={{ selected: p === active }}>
          <Text style={[styles.tabText, { color: p === active ? INK.onAccent : theme.tokens.text.primary }]}>{p}</Text>
        </Pressable>
      ))}
    </ScrollView>
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
  line: { flexDirection: 'row', alignItems: 'center' },
  row: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    minHeight: 52,
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.md,
  },
  stop: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center', borderRadius: theme.base.radius.md },
  port: { flex: 1, fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  error: { fontSize: theme.base.text.sm, paddingHorizontal: theme.base.space[2], paddingBottom: theme.base.space[2] },
  note: { fontSize: theme.base.text.xs, marginTop: theme.base.space[2] },
  tabs: { gap: theme.base.space[2], paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2] },
  tab: { minWidth: 56, minHeight: 44, alignItems: 'center', justifyContent: 'center', paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md },
  tabText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, fontVariant: ['tabular-nums'] },
}));
