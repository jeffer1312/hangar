const mockTheme: any = {
  tokens: {
    bg: { base: '#fff', surface: '#eee', elevated: '#ddd', hover: '#ddd' },
    text: { primary: '#000', secondary: '#666', muted: '#999' },
    border: { subtle: '#ccc', default: '#ccc' },
    status: { warning: '#f90', error: '#f00', success: '#0a0' },
    accent: { base: '#00f', dim: '#eef' },
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
    space: [0, 4, 8, 12, 16, 24] as any,
    text: { xxs: 11, xs: 12, sm: 14, base: 16, lg: 18, xl: 20 } as any,
    radius: { sm: 4, md: 8, lg: 12, xl: 16, full: 999 } as any,
    fontMono: 'monospace',
  },
  panelAlpha: 0.86,
  surfaceAlpha: 1,
};
export const StyleSheet: any = {
  create: (fn: any) => (typeof fn === 'function' ? fn(mockTheme) : fn),
};
export const UnistylesRuntime: any = { setAdaptiveThemes: () => {}, setTheme: () => {}, updateTheme: () => {}, themeName: 'dark' };
export const useUnistyles = () => ({ theme: mockTheme, rt: UnistylesRuntime });
