import { Platform } from 'react-native';

// A escala do app. O tema expõe estes mesmos objetos (`theme.base.*`, `theme.type`); importar daqui
// é para estilo montado fora do alcance do `theme` (componente sem hook, estilo inline).
export const TEXT = { xxxs: 11, xxs: 12, xs: 13, sm: 15, base: 17, lg: 19, xl: 22, xxl: 28 } as const;
export const MONO = Platform.select({ ios: 'Menlo', default: 'monospace' });
// Passo de 4 com dois meios-passos; valor fora daqui arredonda para o vizinho, não vira token.
export const SPACE = { 0.5: 2, 1: 4, 1.5: 6, 2: 8, 3: 12, 4: 16, 5: 20, 6: 24, 8: 32, 10: 40 } as const;
export const RADIUS = { xs: 4, sm: 8, md: 12, lg: 18, xl: 24, full: 9999 } as const;
export const WEIGHT = { normal: '400', medium: '500', semibold: '600', bold: '700' } as const;

// Papel de texto = tamanho + peso + entrelinha num lugar só; a tela espalha (`...theme.type.meta`)
// e só acrescenta cor. Corpo da conversa não usa `body` direto: ele passa pela escala da Aparência.
export const TYPE = {
  title: { fontSize: TEXT.xl, fontWeight: WEIGHT.bold, lineHeight: 28 },
  heading: { fontSize: TEXT.lg, fontWeight: WEIGHT.semibold, lineHeight: 24 },
  body: { fontSize: TEXT.base, fontWeight: WEIGHT.normal, lineHeight: 24 },
  label: { fontSize: TEXT.sm, fontWeight: WEIGHT.medium, lineHeight: 20 },
  control: { fontSize: TEXT.sm, fontWeight: WEIGHT.semibold, lineHeight: 20 },
  meta: { fontSize: TEXT.xs, fontWeight: WEIGHT.normal, lineHeight: 18 },
  caption: { fontSize: TEXT.xxs, fontWeight: WEIGHT.normal, lineHeight: 16 },
  badge: { fontSize: TEXT.xxxs, fontWeight: WEIGHT.bold, lineHeight: 13 },
  mono: { fontSize: TEXT.xs, fontWeight: WEIGHT.normal, lineHeight: 18, fontFamily: MONO },
} as const;

// Cores que não mudam com tema nem acento: texto sobre o destaque/estado e o véu sobre mídia.
export const INK = { onAccent: '#ffffff', scrim: 'rgba(0,0,0,0.6)' } as const;
