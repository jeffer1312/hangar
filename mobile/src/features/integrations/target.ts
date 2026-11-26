import { create } from 'zustand';

/** Pedido de aprovação de integração que a folha deve mostrar (toque no aviso ou no chat). */
export interface IntegrationApprovalTarget { serverId: string; requestId: string | null }

export const useIntegrationApprovalTarget = create<{ target: IntegrationApprovalTarget | null }>(() => ({ target: null }));

export const openIntegrationApproval = (target: IntegrationApprovalTarget | null) =>
  useIntegrationApprovalTarget.setState({ target });
