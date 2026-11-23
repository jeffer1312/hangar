import { create } from 'zustand';
import { prefs } from '../../stores/prefs';

const KEY = 'settings.whatsappSeen';

/** Selo "novo" da entrada WhatsApp: some na primeira abertura da página neste aparelho. */
export const useWhatsAppSeen = create<{ seen: boolean; markSeen: () => void }>((set, get) => ({
  seen: prefs.getBoolean(KEY) ?? false,
  markSeen() {
    if (get().seen) return;
    prefs.set(KEY, true);
    set({ seen: true });
  },
}));
