import type { ComponentType } from 'react';
import type { ViewProps } from 'react-native';
import { requireNativeView, requireOptionalNativeModule } from 'expo';

export { portTunnelSupported } from './support';

export type PortTunnelNavigation = { url: string; title: string; loading: boolean };
/** `code`: o `x-hangar-tunnel` da recusa (`port_tunnel_refused`…), `http_<status>` sem ele, `network`,
 * `listen` (SOCKS não subiu) ou `proxy_override` (WebView recusou o proxy). */
export type PortTunnelFailure = { code: string; description: string };

export type PortTunnelViewProps = ViewProps & {
  /** Um proxy e um armazenamento de cookies por servidor. */
  serverKey: string;
  /** `ws(s)://…/api/port-tunnel?token=…`; o nativo acrescenta `&port=N` a cada conexão. */
  tunnelUrl: string;
  /** `http://localhost:<porta>/…`, como na máquina da sessão. */
  url: string;
  /** Portas da sessão: o SOCKS do aparelho não tem senha, então recusa qualquer outra. */
  ports: number[];
  onNavigation?: (e: { nativeEvent: PortTunnelNavigation }) => void;
  onFailure?: (e: { nativeEvent: PortTunnelFailure }) => void;
};

/** null em binário sem o módulo. */
export const PortTunnelView: ComponentType<PortTunnelViewProps> | null = requireOptionalNativeModule('PortTunnel')
  ? requireNativeView<PortTunnelViewProps>('PortTunnel')
  : null;
