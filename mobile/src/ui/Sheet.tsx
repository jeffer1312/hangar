import { forwardRef, type ReactNode } from 'react';
import { TrueSheet, type SheetDetent } from '@lodev09/react-native-true-sheet';
import { useUnistyles } from 'react-native-unistyles';

export type SheetRef = TrueSheet;
type Size = 'auto' | 'medium' | 'large';
// A API do true-sheet 3.11 é `detents: SheetDetent[]` ('auto' | 'peek' | fração 0..1).
const DETENT: Record<Size, SheetDetent> = { auto: 'auto', medium: 0.5, large: 0.92 };

// true-sheet exige New Arch (ligada: app.json newArchEnabled). Teto de 3 detents: Android trunca em 3.
// Fundo do vidro: a sheet é navegação, então leva o modalAlpha; conteúdo dentro dela usa surfaceAlpha.
export const Sheet = forwardRef<TrueSheet, { sizes?: Size[]; children: ReactNode; onDismiss?: () => void; scrollable?: boolean }>(
  function Sheet({ sizes = ['auto'], children, onDismiss, scrollable }, ref) {
    const { theme, rt } = useUnistyles();
    const [r, g, b] = theme.tokens.glass.panelRgb;
    return (
      <TrueSheet
        ref={ref}
        detents={sizes.slice(0, 3).map((s) => DETENT[s])}
        cornerRadius={theme.base.radius.xl}
        backgroundBlur={rt.themeName === 'dark' ? 'dark' : 'light'}
        backgroundColor={`rgba(${r},${g},${b},${theme.tokens.glass.modalAlpha})`}
        grabber
        scrollable={scrollable}
        onDidDismiss={onDismiss ? () => onDismiss() : undefined}
      >
        {children}
      </TrueSheet>
    );
  },
);
