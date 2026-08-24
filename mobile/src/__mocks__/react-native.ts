import * as React from 'react';

function domProps(props: any) {
  const { style, accessibilityRole, accessibilityLabel, numberOfLines, onPress, ...rest } = props;
  const normalizedStyle = Array.isArray(style) ? Object.assign({}, ...style.filter(Boolean)) : style;
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
export const ActivityIndicator = () => React.createElement('div', null, 'loading');
export const Platform = { OS: 'android', select: (x: any) => x.android ?? x.default };
export const TextInput = (props: any) => React.createElement('textarea', domProps(props));
export const StyleSheet = { create: (x: any) => x, flatten: (x: any) => x };
