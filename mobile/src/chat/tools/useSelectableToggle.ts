import { useRef } from 'react';

/** Props do corpo aberto: toque fecha, toque longo seleciona. Sem a guarda, soltar a seleção também fechava o bloco. */
export function useSelectableToggle(toggle: () => void) {
  const selecting = useRef(false);
  return {
    onPressIn: () => { selecting.current = false; },
    onLongPress: () => { selecting.current = true; },
    onPress: () => { if (!selecting.current) toggle(); },
  };
}
