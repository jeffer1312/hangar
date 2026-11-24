const mockTheme: any = {
  tokens: {
    bg: { base: '#fff', surface: '#eee', elevated: '#ddd', hover: '#ddd' },
    text: { primary: '#000', secondary: '#666', muted: '#999' },
    border: { subtle: '#ccc', default: '#ccc' },
    status: { warning: '#f90', error: '#f00', success: '#0a0' },
    accent: { base: '#00f', dim: '#eef' },
    pill: { input: { fg: '#b70' }, working: { fg: '#70f' }, idle: { fg: '#080' }, dead: { fg: '#888' } },
    bubbleUser: '#def',
    glass: {
      rgb: [38, 36, 44],
      panelRgb: [26, 24, 29],
      solidRgb: [24, 23, 28],
      bgAlpha: 0.46,
      solidAlpha: 0.94,
      panelAlpha: 0.86,
      modalAlpha: 0.93,
      border: '#333',
    },
  },
  base: {
    space: { 0.5: 2, 1: 4, 1.5: 6, 2: 8, 3: 12, 4: 16, 5: 20, 6: 24, 8: 32, 10: 40 } as any,
    weight: { normal: '400', medium: '500', semibold: '600', bold: '700' },
    text: { xxxs: 11, xxs: 12, xs: 13, sm: 15, base: 17, lg: 19, xl: 22, xxl: 28 } as any,
    radius: { sm: 4, md: 8, lg: 12, xl: 16, full: 999 } as any,
    fontMono: 'monospace',
  },
  // Cópia de src/theme/scale.ts: importar de lá puxaria o Platform, que vários testes mockam sem ele.
  type: {
    title: { fontSize: 22, fontWeight: '700', lineHeight: 28 },
    heading: { fontSize: 19, fontWeight: '600', lineHeight: 24 },
    body: { fontSize: 17, fontWeight: '400', lineHeight: 24 },
    label: { fontSize: 15, fontWeight: '500', lineHeight: 20 },
    control: { fontSize: 15, fontWeight: '600', lineHeight: 20 },
    meta: { fontSize: 13, fontWeight: '400', lineHeight: 18 },
    caption: { fontSize: 12, fontWeight: '400', lineHeight: 16 },
    badge: { fontSize: 11, fontWeight: '700', lineHeight: 13 },
    mono: { fontSize: 13, fontWeight: '400', lineHeight: 18, fontFamily: 'monospace' },
  },
  ink: { onAccent: '#ffffff', scrim: 'rgba(0,0,0,0.6)' },
  panelAlpha: 0.86,
  surfaceAlpha: 1,
  conversa: { texto: 1, linha: 1, codigo: 14, coluna: 1 },
};
export const StyleSheet: any = {
  create: (fn: any) => (typeof fn === 'function' ? fn(mockTheme) : fn),
};
export const UnistylesRuntime: any = { setAdaptiveThemes: () => {}, setTheme: () => {}, updateTheme: () => {}, themeName: 'dark' };
export const useUnistyles = () => ({ theme: mockTheme, rt: UnistylesRuntime });
