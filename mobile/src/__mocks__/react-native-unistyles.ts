const mockTheme: any = {
  tokens: {
    bg: { base: '#fff', surface: '#eee', elevated: '#ddd', hover: '#ddd' },
    text: { primary: '#000', secondary: '#666', muted: '#999' },
    border: { subtle: '#ccc', default: '#ccc' },
    status: { warning: '#f90', error: '#f00', success: '#0a0' },
    accent: { base: '#00f', dim: '#eef' },
    bubbleUser: '#def',
  },
  base: {
    space: [0, 4, 8, 12, 16, 24] as any,
    text: { xs: 12, sm: 14, base: 16, lg: 18, xl: 20 } as any,
    radius: { sm: 4, md: 8, lg: 12, full: 999 } as any,
    fontMono: 'monospace',
  },
};
export const StyleSheet: any = {
  create: (fn: any) => (typeof fn === 'function' ? fn(mockTheme) : fn),
};
export const useUnistyles = () => ({ theme: mockTheme });
