// Ponte tipada entre a Sidebar (dona dos workflows pesados) e o SessionTabs (que vive fora da
// <aside> quando ela está recolhida). Sem estado reativo: só registra handlers e delega.
// Sem handler registrado, toda chamada é no-op — nada quebra num layout que não usa as abas.
import type { AggSession } from './types';

export interface SidebarBridgeHandlers {
  openCreate: () => void;
  openSessionMenu: (event: MouseEvent, session: AggSession, serverId: string) => void;
  openKebab: (event: MouseEvent) => void;
}
let handlers: SidebarBridgeHandlers | null = null;

export const sidebarBridge = {
  register(next: SidebarBridgeHandlers) {
    handlers = next;
    return () => { if (handlers === next) handlers = null; };
  },
  openCreate() { handlers?.openCreate(); },
  openSessionMenu(event: MouseEvent, session: AggSession, serverId: string) {
    handlers?.openSessionMenu(event, session, serverId);
  },
  openKebab(event: MouseEvent) { handlers?.openKebab(event); },
};
