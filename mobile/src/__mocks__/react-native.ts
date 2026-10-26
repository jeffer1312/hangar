import * as React from 'react';

function domProps(props: any) {
  const { style, accessibilityRole, accessibilityLabel, numberOfLines, onPress, ...rest } = props;
  // O `Pressable` aceita estilo em função do estado: o teste vê o de repouso.
  const raw = typeof style === 'function' ? style({ pressed: false }) : style;
  const normalizedStyle = Array.isArray(raw) ? Object.assign({}, ...raw.flat(Infinity).filter(Boolean)) : raw;
  return {
    ...rest,
    ...(normalizedStyle ? { style: normalizedStyle } : {}),
    ...(accessibilityRole ? { role: accessibilityRole } : {}),
    ...(accessibilityLabel ? { 'aria-label': accessibilityLabel } : {}),
    ...(onPress ? { onClick: onPress } : {}),
  };
}

export const View = (props: any) => React.createElement('div', domProps(props), props.children);
export const Text = (props: any) => React.createElement('span', domProps(props), props.children);
export const Pressable = (props: any) => React.createElement('button', { ...domProps(props), disabled: props.disabled }, props.children);
export const ScrollView = (props: any) => React.createElement('div', domProps(props), props.children);
export const FlatList = (props: any) => React.createElement('div', null,
  (props.data ?? []).map((item: any, index: number) => React.createElement(React.Fragment,
    { key: props.keyExtractor ? props.keyExtractor(item, index) : index }, props.renderItem({ item, index }))),
  props.ListEmptyComponent && !(props.data ?? []).length
    ? (typeof props.ListEmptyComponent === 'function' ? React.createElement(props.ListEmptyComponent) : props.ListEmptyComponent) : null);
export const ActivityIndicator =() => React.createElement('div', null, 'loading');
// Animação parada: o teste olha o conteúdo, não o movimento.
const animacao = () => ({ start: () => {}, stop: () => {} });
export const Animated = {
  Value: class { constructor(public value: number) {} },
  View, Text,
  timing: animacao, sequence: animacao, loop: animacao,
};
export const Linking = { openURL: (_url: string) => Promise.resolve() };
export const InteractionManager = { runAfterInteractions: (task: () => void) => { const t = setTimeout(task, 0); return { cancel: () => clearTimeout(t) }; } };
export const Platform = { OS: 'android', select: (x: any) => x.android ?? x.default };
export const BackHandler = { addEventListener: () => ({ remove() {} }), exitApp() {} };
export const TextInput = (props: any) => React.createElement('textarea', domProps(props));
export const TurboModuleRegistry = { get: () => null };
export const StyleSheet = { create: (x: any) => x, flatten: (x: any) => x };
export const Modal = (props: any) => (props.visible === false ? null : React.createElement('div', null, props.children));
export const Alert = { alert: (..._args: unknown[]) => {} };
export const AppState = { currentState: 'active', addEventListener: () => ({ remove: () => {} }) };
export const Keyboard ={ dismiss: () => {}, isVisible: () => false, addListener: () => ({ remove: () => {} }) };
export const useWindowDimensions = () => ({ width: 390, height: 844, scale: 3, fontScale: 1 });
export const Dimensions = {
  get: (_dimension: 'window' | 'screen') => ({ width: 390, height: 844, scale: 3, fontScale: 1 }),
  addEventListener: () => ({ remove: () => {} }),
};
export const Appearance = { setColorScheme: (_s: 'light' | 'dark' | 'unspecified') => {} };
export const AccessibilityInfo = {
  isReduceTransparencyEnabled: () => Promise.resolve(false),
  addEventListener: () => ({ remove: () => {} }),
};
