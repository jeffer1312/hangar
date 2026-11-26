import { create } from 'zustand';

/** Rascunho de WhatsApp que a folha deve mostrar (toque no aviso ou no push); `draftId: null` = o primeiro da máquina. */
export interface WhatsAppTarget { serverId: string; draftId: string | null }

export const useWhatsAppTarget = create<{ target: WhatsAppTarget | null }>(() => ({ target: null }));

export const openWhatsAppDraft = (target: WhatsAppTarget | null) => useWhatsAppTarget.setState({ target });
