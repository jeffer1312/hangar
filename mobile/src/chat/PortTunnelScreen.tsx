// Portas da sessão numa WebView do app cujo `localhost` é o da máquina da sessão (`/api/port-tunnel`).
// Várias portas ficam abertas ao mesmo tempo, cada uma na sua aba; trocar de aba não recarrega.
import { useCallback, useMemo, useState } from 'react';
import { ActivityIndicator, Linking, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Stack, useRouter } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import type { Edge } from 'react-native-safe-area-context';
import { useShallow } from 'zustand/react/shallow';
import { baseOf, openPortPreview, PortNotPageError, type Server } from '@hangar/core';
import { PortTunnelView, type PortTunnelFailure } from '../../modules/port-tunnel';
import { Screen } from '../ui/Screen';
import { HeaderBack, useGlassHeader } from '../ui/glassHeader';
import { Icon } from '../ui/Icon';
import { haptic } from '../ui/haptics';
import { toast } from '../ui/Toast';
import { useServers } from '../stores/servers';
import { PortTabs } from './PortsSheet';
import { useSessions } from '../stores/sessions';
import * as m from '../paraglide/messages';

const EDGES: Edge[] = ['bottom', 'left', 'right'];

/** O nativo acrescenta `&port=N`. Mesma credencial e rota do terminal: `baseOf` e `?token=`. */
function tunnelUrl(server: Server): string {
  const base = baseOf(server).replace(/\/+$/, '').replace(/^http/, 'ws');
  return `${base}/api/port-tunnel?token=${encodeURIComponent(server.token)}`;
}

function failureText(port: number, f: PortTunnelFailure): string {
  switch (f.code) {
    case 'port_tunnel_refused': return m.mobile_ports_tunnel_err_nothing({ port: String(port) });
    case 'port_tunnel_port_refused': return m.mobile_ports_tunnel_err_port();
    // O 401/403 do porteiro também chega no cabeçalho, com o código dele em vez do do túnel.
    case 'port_tunnel_token_required': case 'http_401': case 'http_403':
    case 'erro_nao_autorizado': case 'erro_fora_do_convite':
      return m.mobile_ports_tunnel_err_token();
    case 'listen': case 'proxy_override': return m.mobile_ports_tunnel_err_device();
    default: return m.mobile_ports_tunnel_err_network({ detail: f.description || f.code });
  }
}

type PageState = { title: string; loading: boolean; failure: string | null; reloads: number };
const fresh: PageState = { title: '', loading: true, failure: null, reloads: 0 };

export function PortTunnelScreen({ serverId, name, port }: { serverId: string; name: string; port: number }) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const header = useGlassHeader();
  const headerHeight = useHeaderHeight();
  const server = useServers((s) => s.servers.find((x) => x.id === serverId));
  const listed = useSessions(useShallow((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.ports?.map((p) => p.port) ?? []));
  const [opened, setOpened] = useState<number[]>([port]);
  const [active, setActive] = useState(port);
  const [pages, setPages] = useState<Record<number, PageState>>({});
  const [outsideBusy, setOutsideBusy] = useState(false);
  const page = (p: number) => pages[p] ?? fresh;
  const patch = useCallback((p: number, change: Partial<PageState>) =>
    setPages((all) => ({ ...all, [p]: { ...(all[p] ?? fresh), ...change } })), []);

  // Porta que saiu da lista mas está aberta continua na faixa.
  const tabs = useMemo(() => [...new Set([...opened, ...listed])].sort((a, b) => a - b), [opened, listed]);
  const select = (p: number) => {
    haptic.selection();
    setOpened((o) => (o.includes(p) ? o : [...o, p]));
    setActive(p);
  };
  // Remontar a WebView recarrega até a página que falhou antes de existir.
  const reload = useCallback(() => setPages((all) => {
    const cur = all[active] ?? fresh;
    return { ...all, [active]: { ...cur, failure: null, loading: true, reloads: cur.reloads + 1 } };
  }), [active]);

  // "Link de fora": a prévia do Connect (ou Tailscale) de hoje, no navegador do sistema.
  const outside = useCallback(async () => {
    if (!server || outsideBusy) return;
    setOutsideBusy(true);
    try {
      const { url } = await openPortPreview(server, active);
      await Linking.openURL(url);
    } catch (e) {
      toast.erro(e instanceof PortNotPageError ? m.mobile_ports_not_page() : e instanceof Error ? e.message : String(e));
    } finally {
      setOutsideBusy(false);
    }
  }, [server, active, outsideBusy]);

  const back = useCallback(() => router.replace(`/s/${encodeURIComponent(serverId)}/${encodeURIComponent(name)}` as never), [router, serverId, name]);
  const canGoBack = router.canGoBack();
  const c = theme.tokens.text.secondary;
  const current = page(active);
  const headerOptions = useMemo(() => ({
    ...header,
    title: current.title || `localhost:${active}`,
    headerLeft: canGoBack ? undefined : () => <HeaderBack onPress={back} />,
    headerRight: () => (
      <View style={styles.actions}>
        <Pressable onPress={reload} hitSlop={8} style={styles.icon} accessibilityRole="button" accessibilityLabel={m.mobile_ports_tunnel_reload()}>
          {current.loading && !current.failure ? <ActivityIndicator color={c} /> : <Icon name="RotateCw" size={18} color={c} />}
        </Pressable>
        <Pressable onPress={() => void outside()} disabled={outsideBusy} hitSlop={8} style={styles.icon} accessibilityRole="button"
          accessibilityLabel={m.mobile_ports_tunnel_outside()} accessibilityState={{ busy: outsideBusy }}>
          {outsideBusy ? <ActivityIndicator color={c} /> : <Icon name="Link" size={18} color={c} />}
        </Pressable>
      </View>
    ),
  }), [header, current.title, current.loading, current.failure, active, canGoBack, back, reload, outside, outsideBusy, c]);

  let body: React.ReactNode;
  if (!server || !PortTunnelView) {
    body = <View style={styles.center}><Text style={styles.note} accessibilityRole="alert">{m.mobile_ports_tunnel_unavailable()}</Text></View>;
  } else {
    const TunnelView = PortTunnelView;
    const url = tunnelUrl(server);
    body = opened.map((p) => {
      const state = page(p);
      return (
        <View key={p} style={[styles.flex, p !== active && styles.hidden]}>
          <TunnelView
            key={state.reloads}
            style={styles.flex}
            serverKey={server.id}
            tunnelUrl={url}
            url={`http://localhost:${p}/`}
            ports={tabs}
            onNavigation={(e) => patch(p, { title: e.nativeEvent.title, loading: e.nativeEvent.loading })}
            onFailure={(e) => {
              haptic.error();
              patch(p, { loading: false, failure: failureText(p, e.nativeEvent) });
            }}
          />
          {state.failure ? (
            <View style={[StyleSheet.absoluteFill, styles.center, { backgroundColor: theme.tokens.bg.base }]} accessibilityRole="alert">
              <Text style={styles.note} selectable>{state.failure}</Text>
              <View style={styles.row}>
                <Pressable onPress={reload} style={styles.button} accessibilityRole="button">
                  <Text style={styles.buttonText}>{m.mobile_ports_tunnel_retry()}</Text>
                </Pressable>
                <Pressable onPress={() => void outside()} style={styles.button} accessibilityRole="button">
                  <Text style={styles.buttonText}>{m.mobile_ports_tunnel_outside()}</Text>
                </Pressable>
              </View>
            </View>
          ) : null}
        </View>
      );
    });
  }

  return (
    <Screen edges={EDGES}>
      <Stack.Screen options={headerOptions} />
      <View style={[styles.flex, { paddingTop: headerHeight }]}>
        {tabs.length > 1 ? <PortTabs tabs={tabs} active={active} onSelect={select} /> : null}
        <View style={styles.flex}>{body}</View>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  flex: { flex: 1 },
  hidden: { display: 'none' },
  center: { flex: 1, justifyContent: 'center', alignItems: 'center', gap: theme.base.space[3], padding: theme.base.space[6] },
  note: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center' },
  row: { flexDirection: 'row', gap: theme.base.space[3] },
  button: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[3] },
  buttonText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.accent.base },
  actions: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  icon: { width: 40, height: 44, alignItems: 'center', justifyContent: 'center' },
}));
