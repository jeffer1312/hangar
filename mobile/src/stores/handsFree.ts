import { create } from 'zustand';
import { prefs } from './prefs';

const KEY = 'ditado.maosLivres';

type HandsFree = { on: boolean; set: (on: boolean) => void };

// Do aparelho, como o `hands_free` do PC: nasce desligado e só a pessoa liga.
export const useHandsFree = create<HandsFree>((set) => ({
  on: prefs.getBoolean(KEY) ?? false,
  set(on) {
    prefs.set(KEY, on);
    set({ on });
  },
}));
