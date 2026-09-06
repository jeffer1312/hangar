import { StyleSheet } from 'react-native-unistyles';
import { themeDark, themeLight, themeBase } from '@hangar/core';
import { prefs } from '../stores/prefs';

// Lê o MMKV cru, e não o store de aparência: este módulo é o primeiro import do _layout e precisa
// rodar antes de qualquer coisa tocar o UnistylesRuntime — o store importa `aplicarMaterial`, que
// mexe no runtime. Sem semear daqui, tudo pintava com alpha de fábrica até o primeiro
// `aplicarMaterial()`, e a tela abria com um piscão de opacidade errada.
const alphaSalvo = (chave: string, padrao: number) => {
  const v = prefs.getNumber(chave);
  return typeof v === 'number' && Number.isFinite(v) ? Math.max(0, Math.min(1, v)) : padrao;
};

const mk = (t: typeof themeDark) => ({
  tokens: t,
  base: themeBase,
  panelAlpha: alphaSalvo('aparencia.panelAlpha', t.glass.panelAlpha),
  surfaceAlpha: alphaSalvo('aparencia.surfaceAlpha', 1),
});

const appThemes = { light: mk(themeLight), dark: mk(themeDark) };

type AppThemes = typeof appThemes;

declare module 'react-native-unistyles' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  export interface UnistylesThemes extends AppThemes {}
}

StyleSheet.configure({ themes: appThemes, settings: { adaptiveThemes: true } });
