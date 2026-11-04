import { useRef } from 'react';
import { Text, View } from 'react-native';

// Animação não roda no teste: o valor final entra direto e o estilo animado vira estilo comum.
const passa = (v: any) => v;
export const useSharedValue = (v: any) => useRef({ value: v }).current;
export const useDerivedValue = (fn: () => any) => ({ value: fn() });
export const useAnimatedStyle = (fn: () => any) => fn();
export const useAnimatedProps = (fn: () => any) => fn();
export const useReducedMotion = () => true;
export const useAnimatedReaction = () => {};
export const useFrameCallback = () => ({ setActive: () => {} });
export const withTiming = passa;
export const withRepeat = passa;
export const cancelAnimation = () => {};
export const withSpring = passa;
export const withDelay = (_ms: number, v: any) => v;
export const withSequence = (...vs: any[]) => vs[vs.length - 1];
export const interpolate = (_v: number, _i: number[], out: number[]) => out[0];
export const interpolateColor = (_v: number, _i: number[], out: string[]) => out[0];
export const Easing: any = new Proxy({}, { get: (_target, key) => key === 'bezierFn' ? () => passa : (...a: any[]) => a[0] ?? passa });
export const FadeIn: any = { duration: () => FadeIn, delay: () => FadeIn };
// Construtor de animação de layout: toda chamada encadeada devolve o mesmo objeto.
const layoutBuilder = (): any => new Proxy({}, { get: (target, key) => key === 'then' ? undefined : () => target });
export const FadeInDown: any = layoutBuilder();
export const FadeOut: any = layoutBuilder();
export const LinearTransition: any = layoutBuilder();
export type SharedValue<T = any> = { value: T };

const Animated = { View, Text, createAnimatedComponent: passa };
export default Animated;
