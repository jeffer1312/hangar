import { UnistylesRuntime } from 'react-native-unistyles';
import { comAcento, themeDark, themeLight } from '@hangar/core';

// Deriva SEMPRE dos tokens de fábrica, nunca do tema atual: acumular acento sobre acento faria a
// cor derivar a cada troca, e tirar o acento não teria como voltar ao original.
export function aplicarMaterial(s: { panelAlpha: number; surfaceAlpha: number; acento: string | null }) {
  for (const [nome, base] of [['light', themeLight], ['dark', themeDark]] as const) {
    UnistylesRuntime.updateTheme(nome, (t) => ({
      ...t,
      tokens: comAcento(base, s.acento),
      panelAlpha: s.panelAlpha,
      surfaceAlpha: s.surfaceAlpha,
    }));
  }
}
