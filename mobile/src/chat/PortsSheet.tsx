// Portas que os processos da sessão abriram. Tocar abre a página no app pelo túnel; aparelho sem suporte
// (iOS < 17, WebView sem PROXY_OVERRIDE, binário antigo) abre a prévia no navegador do sistema.
import { useRef, useState, type ReactNode } from 'react';
import { Alert, Linking } from 'react-native';
import { useRouter } from 'expo-router';
import { useShallow } from 'zustand/react/shallow';
import { openPortPreview, PortNotPageError, stopPortServer, type PortEntry } from '@hangar/core';
import { portTunnelSupported } from '../../modules/port-tunnel/support';
import { Sheet } from '../ui/Sheet';
import { haptic } from '../ui/haptics';
import { useServers } from '../stores/servers';
import { useSessions } from '../stores/sessions';
import { PortRow, PortsBody, PortsLoading, PortsStatus } from './PortsPieces';
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

export function PortsSheet({ open, onClose, serverId, name }: { open: boolean; onClose: () => void; serverId: string; name: string }) {
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
    body = <PortsLoading />;
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
