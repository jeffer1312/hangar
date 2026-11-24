import { useSyncExternalStore } from 'react';
import { baseOf, subscribeRoutes, type Server } from '@hangar/core';

/**
 * Endereço em uso para o servidor, relido quando a rota muda (rede local ↔ Tailscale, rota decidida
 * depois de a bolha montar). Quem monta URL de mídia põe isto nas dependências do memo.
 */
export function useRouteBase(server: Server | null | undefined): string {
  return useSyncExternalStore(subscribeRoutes, () => (server ? baseOf(server) : ''));
}
