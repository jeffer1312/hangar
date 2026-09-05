import { forwardRef } from 'react';
import { TextInput, type TextInputProps } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';

// Campo multilinha do app: cresce até `maxHeight` e daí rola. Fundo é do contêiner (o composer
// carrega o vidro), por isso o input não pinta nada.
export const MultilineInput = forwardRef<TextInput, TextInputProps & { maxHeight?: number; mono?: boolean }>(
  function MultilineInput({ maxHeight = 120, mono, style, ...rest }, ref) {
    const { theme } = useUnistyles();
    return (
      <TextInput
        ref={ref}
        multiline
        scrollEnabled
        placeholderTextColor={theme.tokens.text.muted}
        style={[styles.input, { maxHeight, color: theme.tokens.text.primary }, mono && { fontFamily: theme.base.fontMono }, style]}
        {...rest}
      />
    );
  },
);

const styles = StyleSheet.create((theme) => ({
  input: { fontSize: theme.base.text.base, paddingVertical: 8, paddingHorizontal: 10, textAlignVertical: 'top' },
}));
