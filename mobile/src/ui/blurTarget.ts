import { createContext, useContext, type RefObject } from 'react';
import type { View } from 'react-native';

// No Android o BlurView só desfoca de verdade quando recebe a ref de um BlurTargetView que
// envolve o que está atrás dele; sem ela ele avisa no log e cai em "none". Quem envolve é a
// `Screen`, e o `Glass` está fundo demais na árvore pra receber isso por prop.
export const BlurTargetContext = createContext<RefObject<View | null> | null>(null);

export const useBlurTarget = () => useContext(BlurTargetContext);
