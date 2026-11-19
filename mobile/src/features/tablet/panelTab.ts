import { create } from 'zustand';

export type PanelTab = 'context' | 'files' | 'review' | 'background' | 'git';

type PanelTabRequest = {
  /** `serverId::name` da sessão que deve abrir com o painel na aba pedida. */
  request: { key: string; tab: PanelTab } | null;
  open: (serverId: string, name: string, tab: PanelTab) => void;
  clear: (key: string) => void;
};

// Sem painel para consumir (layout sem ele), o pedido não pode esperar e abrir a sessão em Revisão mais tarde.
export const PANEL_TAB_REQUEST_TTL_MS = 3000;
let expiry: ReturnType<typeof setTimeout> | undefined;

// Pedido de uma vez: o chip pede antes de a rota trocar, e o painel da sessão o consome ao montar.
export const usePanelTabRequest = create<PanelTabRequest>((set, get) => ({
  request: null,
  open: (serverId, name, tab) => {
    const request = { key: `${serverId}::${name}`, tab };
    set({ request });
    clearTimeout(expiry);
    expiry = setTimeout(() => { if (get().request === request) set({ request: null }); }, PANEL_TAB_REQUEST_TTL_MS);
  },
  clear: (key) => { if (get().request?.key === key) set({ request: null }); },
}));
