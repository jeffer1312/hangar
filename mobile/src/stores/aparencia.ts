import { create } from 'zustand';
import { UnistylesRuntime } from 'react-native-unistyles';
import { prefs } from './prefs';

export type Tema = 'system' | 'light' | 'dark';
const TEMAS: Tema[] = ['system', 'light', 'dark'];
const K = 'aparencia.tema';

function ler(): Tema {
  const v = prefs.getString(K) as Tema | undefined;
  return v && TEMAS.includes(v) ? v : 'system';
}

// Unistyles: `adaptiveThemes` segue o SO; fixar tema exige desligar o adaptativo antes de setTheme.
function aplicar(t: Tema) {
  if (t === 'system') UnistylesRuntime.setAdaptiveThemes(true);
  else { UnistylesRuntime.setAdaptiveThemes(false); UnistylesRuntime.setTheme(t); }
}

export const useAparencia = create<{ tema: Tema; setTema: (t: Tema) => void }>((set) => ({
  tema: ler(),
  // aplicar primeiro: se o Unistyles falhar, nada persiste — senão o cold start seguinte repete a falha.
  setTema: (t) => { aplicar(t); prefs.set(K, t); set({ tema: t }); },
}));

export function aplicarTemaSalvo() { aplicar(useAparencia.getState().tema); }
