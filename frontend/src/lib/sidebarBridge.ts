// Ponte tipada entre a Sidebar (dona dos workflows pesados) e o SessionTabs (que vive fora da
// <aside> quando ela está recolhida). Sem estado reativo: só registra handlers e delega.
// Sem handler registrado, toda chamada é no-op — nada quebra num layout que não usa as abas.
import type { AggSession } from '@hangar/core';

export interface SidebarBridgeHandlers {
  openCreate: () => void;
  openSessionMenu: (event: MouseEvent, session: AggSession, serverId: string) => void;
  openKebab: (event: MouseEvent) => void;
}
let handlers: SidebarBridgeHandlers | null = null;

// Canal SEPARADO de foco de aba (round 2): quem RENDERIZA as abas (SessionTabs) registra o
// handler de foco — a Sidebar só pede foco pra aba recriada após um rename. Canal próprio porque
// o slot de workflows é da Sidebar e o slot de foco é do SessionTabs (direções opostas).
export interface TabFocusHandler {
  focusTab: (key: string) => void;
}
let tabFocus: TabFocusHandler | null = null;

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
  registerTabFocus(next: TabFocusHandler) {
    tabFocus = next;
    return () => { if (tabFocus === next) tabFocus = null; };
  },
  focusTab(key: string) { tabFocus?.focusTab(key); },
};
