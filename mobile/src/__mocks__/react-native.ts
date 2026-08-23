import * as React from 'react';
export const View = (props: any) => React.createElement('div', props, props.children);
export const Text = (props: any) => React.createElement('span', props, props.children);
export const Pressable = (props: any) => React.createElement('button', { onClick: props.onPress, disabled: props.disabled }, props.children);
export const ScrollView = (props: any) => React.createElement('div', props, props.children);
export const ActivityIndicator = () => React.createElement('div', null, 'loading');
export const Platform = { OS: 'android', select: (x: any) => x.android ?? x.default };
export const TextInput = (props: any) => React.createElement('textarea', props);
export const StyleSheet = { create: (x: any) => x, flatten: (x: any) => x };
