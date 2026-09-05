import { forwardRef, useEffect, useImperativeHandle, useRef, type ReactNode } from 'react';
import { TrueSheet, type SheetDetent } from '@lodev09/react-native-true-sheet';
import { useUnistyles } from 'react-native-unistyles';
import { alphaDoVidro, useReduceTransparency } from './Glass';
import { toast } from './Toast';
import * as m from '../paraglide/messages';

export type SheetRef = TrueSheet;
type Size = 'auto' | 'medium' | 'large';
// A API do true-sheet 3.11 é `detents: SheetDetent[]` ('auto' | 'peek' | fração 0..1).
const DETENT: Record<Size, SheetDetent> = { auto: 'auto', medium: 0.5, large: 0.92 };

type Props = {
  sizes?: Size[];
  children: ReactNode;
  open?: boolean;
  onDismiss?: () => void;
  scrollable?: boolean;
};

// true-sheet exige New Arch (ligada: app.json newArchEnabled). Teto de 3 detents: Android trunca em 3.
// Fundo do vidro: a sheet é navegação, então leva o modalAlpha; conteúdo dentro dela usa surfaceAlpha.
export const Sheet = forwardRef<TrueSheet, Props>(function Sheet({ sizes = ['auto'], open, children, onDismiss, scrollable }, ref) {
  const { theme, rt } = useUnistyles();
  const reduzir = useReduceTransparency();
  const innerRef = useRef<TrueSheet>(null);
  // present()/dismiss() nativos rejeitam se a view nunca montou (lazy no 1º present); rastreia pra não
  // chamar dismiss() antes de qualquer present(), e pra saber quando reapresentar após swipe-to-close.
  const apresentada = useRef(false);

  useImperativeHandle(ref, () => innerRef.current as TrueSheet);

  useEffect(() => {
    if (open === undefined) return; // sem a prop, controle é só imperativo (ref.current.present()/.dismiss())
    if (open) {
      innerRef.current
        ?.present()
        .then(() => { apresentada.current = true; })
        .catch((e: unknown) => {
          console.error('Sheet: present() falhou', e);
          toast.erro(m.erro_desconhecido());
        });
    } else if (apresentada.current) {
      innerRef.current?.dismiss().catch((e: unknown) => console.error('Sheet: dismiss() falhou', e));
    }
  }, [open]);

  const [r, g, b] = theme.tokens.glass.panelRgb;
  return (
    <TrueSheet
      ref={innerRef}
      detents={sizes.slice(0, 3).map((s) => DETENT[s])}
      cornerRadius={theme.base.radius.xl}
      backgroundBlur={reduzir ? undefined : rt.themeName === 'dark' ? 'dark' : 'light'}
      backgroundColor={reduzir ? `rgb(${r},${g},${b})` : `rgba(${r},${g},${b},${alphaDoVidro(theme, 'modal')})`}
      grabber
      scrollable={scrollable}
      onDidDismiss={() => {
        apresentada.current = false;
        onDismiss?.();
      }}
    >
      {children}
    </TrueSheet>
  );
});
