import { createContext, useCallback, useContext, useEffect, useRef, type ReactNode } from 'react';
import { View, type StyleProp, type ViewStyle } from 'react-native';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { anchorMoved, registerAnchor, unregisterAnchor } from './anchors';

/** Peças montadas mas escondidas por outra camada da mesma tela (a lista sob o formulário de nova conversa). */
export const TourHidden = createContext(false);

/**
 * Registra uma peça que o tour aponta; `id` nulo não registra (a mesma peça fora da tela real ou escondida).
 * Tela sem foco (a inicial por baixo da conversa) tira as peças dela: o balão não pode apontar o que está coberto.
 */
export function useTourAnchor(id: string | null) {
  // `WorkingMarkPaused` é o "esta tela perdeu o foco" que a inicial e a conversa já publicam.
  const paused = useContext(WorkingMarkPaused);
  const hidden = useContext(TourHidden);
  const covered = paused || hidden;
  const key = covered ? null : id;
  const node = useRef<View | null>(null);
  const ref = useCallback((view: View | null) => {
    const previous = node.current;
    node.current = view;
    if (!key) return;
    if (previous && previous !== view) unregisterAnchor(key, previous);
    if (view) registerAnchor(key, view);
  }, [key]);
  useEffect(() => {
    const view = node.current;
    if (!key || !view) return;
    registerAnchor(key, view);
    return () => unregisterAnchor(key, view);
  }, [key]);
  const onLayout = useCallback(() => { if (key) anchorMoved(key); }, [key]);
  return { ref, onLayout };
}

/** Embrulha a peça; a ação do passo é avisada onde ela de fato acontece, não no toque que pode ter sido arrasto. */
export function TourAnchor({ id, style, children }: { id: string | null; style?: StyleProp<ViewStyle>; children: ReactNode }) {
  const { ref, onLayout } = useTourAnchor(id);
  return <View ref={ref} collapsable={false} style={style} onLayout={onLayout}>{children}</View>;
}
