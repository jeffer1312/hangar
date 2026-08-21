// packages/core/src/theme.ts — espelho de frontend/src/app.css (blocos :root dark, [data-theme=light], estrutural, easing).
// Mudou no app.css? Mude aqui no mesmo commit (theme.test.ts confere os pares que mais doem).
export interface ThemeTokens {
  bg: { base: string; surface: string; elevated: string; hover: string };
  veuRgb: [number, number, number];
  border: { subtle: string; default: string; strong: string };
  fillSubtle: string;
  text: { primary: string; secondary: string; muted: string; inverse: string };
  accent: { base: string; dim: string; press: string };
  chart: [string, string, string, string];
  bubbleUser: string;
  glass: {
    panelRgb: [number, number, number];
    rgb: [number, number, number];
    solidRgb: [number, number, number];
    bgAlpha: number;
    solidAlpha: number;
    panelAlpha: number;
    modalAlpha: number;
    border: string;
    highlight: string;
    specular: string;
    shadow: string;
  };
  status: { success: string; error: string; warning: string };
  pill: Record<'working' | 'idle' | 'dead' | 'input', { bg: string; fg: string }>;
}
export const dark: ThemeTokens = {
  bg: { base: '#100e11', surface: '#1a171a', elevated: '#221d22', hover: '#2a242a' },
  veuRgb: [16, 14, 17],
  border: {
    subtle: 'rgba(255,248,244,0.07)',
    default: 'rgba(255,248,244,0.12)',
    strong: 'rgba(255,248,244,0.22)',
  },
  fillSubtle: 'rgba(255,248,244,0.055)',
  text: { primary: '#d2cbcd', secondary: '#a0989b', muted: '#8d8489', inverse: '#100e11' },
  accent: { base: '#7c87e8', dim: 'rgba(124,135,232,0.18)', press: '#6e79d6' },
  chart: ['#7c87e8', '#d95926', '#199e70', '#c98500'],
  bubbleUser: '#2b2a2e',
  glass: {
    panelRgb: [26, 24, 29],
    rgb: [38, 36, 44],
    solidRgb: [24, 23, 28],
    bgAlpha: 0.46,
    solidAlpha: 0.94,
    panelAlpha: 0.86,
    modalAlpha: 0.93,
    border: 'rgba(255,255,255,0.10)',
    highlight: 'rgba(255,255,255,0.16)',
    specular: 'rgba(255,255,255,0.30)',
    shadow: 'rgba(0,0,0,0.42)',
  },
  status: { success: '#34c759', error: '#ff453a', warning: '#ff9f0a' },
  pill: {
    working: { bg: 'rgba(124,135,232,0.16)', fg: '#aab2f3' },
    idle: { bg: 'rgba(52,199,89,0.12)', fg: '#34c759' },
    dead: { bg: 'rgba(255,69,58,0.12)', fg: '#ff453a' },
    input: { bg: 'rgba(255,159,10,0.12)', fg: '#ff9f0a' },
  },
};
export const light: ThemeTokens = {
  bg: { base: '#f8f6f2', surface: '#fffdfa', elevated: '#f0ebe3', hover: '#e9e3da' },
  veuRgb: [250, 247, 243],
  border: {
    subtle: 'rgba(50,40,35,0.08)',
    default: 'rgba(50,40,35,0.14)',
    strong: 'rgba(50,40,35,0.24)',
  },
  fillSubtle: 'rgba(50,40,35,0.055)',
  text: { primary: '#221d1b', secondary: '#5f564f', muted: '#6f6660', inverse: '#fffdfa' },
  accent: { base: '#5b6ad0', dim: 'rgba(91,106,208,0.12)', press: '#4d5bc0' },
  chart: ['#5b6ad0', '#eb6834', '#1baf7a', '#eda100'],
  bubbleUser: '#e8e5e0',
  glass: {
    panelRgb: [255, 253, 250],
    rgb: [255, 254, 252],
    solidRgb: [255, 253, 250],
    bgAlpha: 0.52,
    solidAlpha: 0.96,
    panelAlpha: 0.90,
    modalAlpha: 0.96,
    border: 'rgba(40,32,28,0.10)',
    highlight: 'rgba(255,255,255,0.85)',
    specular: 'rgba(255,255,255,0.95)',
    shadow: 'rgba(60,50,45,0.13)',
  },
  status: { success: '#146c30', error: '#b3251c', warning: '#7a5408' },
  pill: {
    working: { bg: 'rgba(91,106,208,0.14)', fg: '#4453b8' },
    idle: { bg: 'rgba(40,160,70,0.14)', fg: '#146c30' },
    dead: { bg: 'rgba(220,50,40,0.12)', fg: '#c4291f' },
    input: { bg: 'rgba(200,130,10,0.14)', fg: '#7a5408' },
  },
};
export const base = {
  fontUi: 'System',
  fontMono: 'JetBrainsMono Nerd Font',
  text: { xxxs: 10, xxs: 11, xs: 12, sm: 14, base: 16, lg: 18, xl: 20 }, // rem×16
  space: { 1: 4, 2: 8, 3: 12, 4: 16, 5: 20, 6: 24, 8: 32, 10: 40 },
  weight: { normal: '400', medium: '500', semibold: '600', bold: '700' } as const,
  radius: { xs: 8, sm: 6, md: 12, lg: 18, xl: 24, full: 9999 },
  easing: {
    out: [0.23, 1, 0.32, 1],
    inOut: [0.77, 0, 0.175, 1],
    drawer: [0.32, 0.72, 0, 1],
    spring: [0.34, 1.56, 0.64, 1],
  },
} as const;
export const theme = { dark, light, base };
