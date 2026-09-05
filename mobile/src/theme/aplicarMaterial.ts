import { UnistylesRuntime } from 'react-native-unistyles';
import { comAcento, themeDark, themeLight } from '@hangar/core';

// Deriva SEMPRE dos tokens de fábrica, nunca do tema atual: acumular acento sobre acento faria a
// cor derivar a cada troca, e tirar o acento não teria como voltar ao original.
// `reduzir` = "Reduzir transparência" do sistema: cola os dois alphas em 1 e some com o vidro de
// tudo de uma vez. Vem por argumento, não do estado, porque não é preferência do app e não persiste.
export function aplicarMaterial(
  s: { panelAlpha: number; surfaceAlpha: number; acento: string | null },
  { reduzir = false }: { reduzir?: boolean } = {},
) {
  for (const [nome, base] of [['light', themeLight], ['dark', themeDark]] as const) {
    UnistylesRuntime.updateTheme(nome, (t) => ({
      ...t,
      tokens: comAcento(base, s.acento),
      panelAlpha: reduzir ? 1 : s.panelAlpha,
      surfaceAlpha: reduzir ? 1 : s.surfaceAlpha,
    }));
  }
}
