// Derivação da cor de acento escolhida pelo usuário: de UM hex saem os três tons do accent e a
// pílula "working", que precisam andar juntos — escolher os quatro à mão sai de sincronia.
import type { ThemeTokens } from './theme';

const HEX = /^#([0-9a-f]{6})$/i;

export function hexParaRgb(hex: string): [number, number, number] | null {
  const m = HEX.exec(hex);
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

const clamp = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
const rgbParaHex = ([r, g, b]: [number, number, number]) =>
  '#' + [r, g, b].map((c) => clamp(c).toString(16).padStart(2, '0')).join('');

// dim = 18% do acento, press = acento 10% mais escuro, pill.working = acento a 16% de fundo com o
// texto clareado 20%. Hex inválido devolve os tokens intactos — sem acento é melhor que acento NaN.
export function comAcento(tokens: ThemeTokens, hex: string | null): ThemeTokens {
  const rgb = hex ? hexParaRgb(hex) : null;
  if (!rgb) return tokens;
  const [r, g, b] = rgb;
  return {
    ...tokens,
    accent: {
      base: rgbParaHex(rgb),
      dim: `rgba(${r},${g},${b},0.18)`,
      press: rgbParaHex([r * 0.9, g * 0.9, b * 0.9]),
    },
    pill: {
      ...tokens.pill,
      working: {
        bg: `rgba(${r},${g},${b},0.16)`,
        fg: rgbParaHex([r + (255 - r) * 0.2, g + (255 - g) * 0.2, b + (255 - b) * 0.2]),
      },
    },
  };
}
