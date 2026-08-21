import { StyleSheet } from 'react-native-unistyles';
import { themeDark, themeLight, themeBase } from '@hangar/core';
import { happyColors } from './mapHappy';

const mk = (t: typeof themeDark) => ({
  tokens: t,
  base: themeBase,
  colors: happyColors(t),
  panelAlpha: t.glass.panelAlpha,
  surfaceAlpha: 1,
});

const appThemes = { light: mk(themeLight), dark: mk(themeDark) };

type AppThemes = typeof appThemes;

declare module 'react-native-unistyles' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  export interface UnistylesThemes extends AppThemes {}
}

StyleSheet.configure({ themes: appThemes, settings: { adaptiveThemes: true } });
