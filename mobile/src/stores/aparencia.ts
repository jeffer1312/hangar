import { create } from 'zustand';
import { UnistylesRuntime } from 'react-native-unistyles';
import { PENSAMENTO_TOOLS, type PensamentoTools, type GroupBy } from '@hangar/core';
import { prefs } from './prefs';

export type Tema = 'system' | 'light' | 'dark';
const TEMAS: Tema[] = ['system', 'light', 'dark'];
const AGRUPAR: GroupBy[] = ['none', 'server', 'project'];
const K = 'aparencia.tema';
const K_PENSAMENTO = 'aparencia.pensamentoTools';
const K_AGRUPAR = 'lista.agrupar';

function ler(): Tema {
  const v = prefs.getString(K) as Tema | undefined;
  return v && TEMAS.includes(v) ? v : 'system';
}

// Valor desconhecido cai no padrão em vez de virar um quarto modo: escrito por versão futura,
// um `if` que não casa com nenhum ramo deixaria a conversa sem bloco de pensamento nenhum.
function lerPensamento(): PensamentoTools {
  const v = prefs.getString(K_PENSAMENTO) as PensamentoTools | undefined;
  return v && PENSAMENTO_TOOLS.includes(v) ? v : 'busca';
}

function lerAgrupar(): GroupBy {
  const v = prefs.getString(K_AGRUPAR) as GroupBy | undefined;
  return v && AGRUPAR.includes(v) ? v : 'server';
}

// Unistyles: `adaptiveThemes` segue o SO; fixar tema exige desligar o adaptativo antes de setTheme.
function aplicar(t: Tema) {
  if (t === 'system') UnistylesRuntime.setAdaptiveThemes(true);
  else { UnistylesRuntime.setAdaptiveThemes(false); UnistylesRuntime.setTheme(t); }
}

interface Aparencia {
  tema: Tema;
  setTema: (t: Tema) => void;
  pensamentoTools: PensamentoTools;
  setPensamentoTools: (v: PensamentoTools) => void;
  agrupar: GroupBy;
  setAgrupar: (v: GroupBy) => void;
}

export const useAparencia = create<Aparencia>((set) => ({
  tema: ler(),
  // aplicar primeiro: se o Unistyles falhar, nada persiste — senão o cold start seguinte repete a falha.
  setTema: (t) => { aplicar(t); prefs.set(K, t); set({ tema: t }); },
  pensamentoTools: lerPensamento(),
  setPensamentoTools: (v) => { prefs.set(K_PENSAMENTO, v); set({ pensamentoTools: v }); },
  agrupar: lerAgrupar(),
  setAgrupar: (v) => { prefs.set(K_AGRUPAR, v); set({ agrupar: v }); },
}));

export function aplicarTemaSalvo() { aplicar(useAparencia.getState().tema); }
