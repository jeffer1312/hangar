import { useCallback, useEffect, useRef, type ReactNode } from 'react';
import { View, type StyleProp, type ViewStyle } from 'react-native';
import { anchorMoved, registerAnchor, reportTourAction, unregisterAnchor } from './anchors';

/** Registra uma peça que o tour aponta; `id` nulo não registra (a mesma peça fora da tela real ou escondida). */
export function useTourAnchor(id: string | null) {
  const node = useRef<View | null>(null);
  const ref = useCallback((view: View | null) => { node.current = view; if (id && view) registerAnchor(id, view); }, [id]);
  useEffect(() => {
    const view = node.current;
    if (id && view) registerAnchor(id, view);
    return () => { if (id && view) unregisterAnchor(id, view); };
  }, [id]);
  const onLayout = useCallback(() => { if (id) anchorMoved(id); }, [id]);
  return { ref, onLayout };
}

/** Embrulha a peça; `step` faz o toque nela contar como a ação do passo. */
export function TourAnchor({ id, step, style, children }: { id: string | null; step?: string; style?: StyleProp<ViewStyle>; children: ReactNode }) {
  const { ref, onLayout } = useTourAnchor(id);
  return <View ref={ref} collapsable={false} style={style} onLayout={onLayout}
    onTouchEnd={id && step ? () => reportTourAction(step) : undefined}>{children}</View>;
}
