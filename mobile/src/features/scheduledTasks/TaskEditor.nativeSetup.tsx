import { createElement, useEffect, type ReactNode, type Ref } from 'react';
import { vi } from 'vitest';

// Só fronteiras nativas: os componentes e os módulos do Hangar permanecem reais.
vi.mock('react-native', async original => {
  const native = await original<typeof import('react-native')>();
  const view = ({ onLayout, ...props }: Record<string, any>) => {
    useEffect(() => { onLayout?.({ nativeEvent: { layout: { x: 0, y: 0, width: 390, height: 844 } } }); }, []);
    return createElement(native.View, props);
  };
  const assign = (ref: Ref<unknown> | undefined, node: HTMLButtonElement | null) => {
    if (node) Object.assign(node, { measureInWindow: (done: (...args: number[]) => void) => done(0, 400, 390, 44) });
    if (typeof ref === 'function') ref(node);
    else if (ref) ref.current = node;
  };
  return { ...native, View: view,
    Pressable: ({ ref, ...props }: Record<string, any>) => createElement(native.Pressable, { ...props, ref: (node: unknown) => assign(ref, node as HTMLButtonElement | null) }),
    TextInput: ({ value, onChangeText, accessibilityLabel, editable }: { value: string; onChangeText: (s: string) => void; accessibilityLabel: string; editable?: boolean }) => createElement('textarea', { value, onInput: (e: React.FormEvent<HTMLTextAreaElement>) => onChangeText(e.currentTarget.value), 'aria-label': accessibilityLabel, disabled: editable === false }),
  };
});
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: ReactNode }) => children }));
vi.mock('expo-router', () => ({ Stack: { Screen: () => null }, useNavigation: () => ({ dispatch: vi.fn() }), router: { push: vi.fn() } }));
vi.mock('expo-router/react-navigation', () => ({ usePreventRemove: vi.fn(), useHeaderHeight: () => 44 }));
vi.mock('react-native-keyboard-controller', async () => ({ KeyboardAvoidingView: (await import('react-native')).View, useReanimatedKeyboardAnimation: () => ({ height: { value: 0 }, progress: { value: 0 } }) }));
vi.mock('expo-blur', async () => ({ BlurTargetView: (await import('react-native')).View, BlurView: (await import('react-native')).View }));
vi.mock('expo-linear-gradient', async () => ({ LinearGradient: (await import('react-native')).View }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, getNumber: () => undefined, getBoolean: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('expo-file-system', () => ({ Directory: class {}, File: class {}, Paths: { cache: '/tmp', document: '/tmp' } }));
vi.mock('@lodev09/react-native-true-sheet', () => ({ TrueSheet: () => null }));
vi.mock('sonner-native', () => ({ toast: { error: vi.fn(), success: vi.fn() }, Toaster: () => null }));
