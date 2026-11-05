import { create } from 'zustand';

/** Pedido do cofre que a folha deve mostrar (toque no aviso ou no push). */
export interface VaultTarget { serverId: string; requestId: string | null }

export const useVaultTarget = create<{ target: VaultTarget | null }>(() => ({ target: null }));

export const openVaultRequest = (target: VaultTarget | null) => useVaultTarget.setState({ target });
