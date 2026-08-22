import * as React from 'react';
import { Text } from 'react-native';
export function MarkdownView(props: { markdown: string }) {
  return <Text>{props.markdown}</Text>;
}
