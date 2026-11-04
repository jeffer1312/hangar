import { Pressable, Text, TextInput, View, type TextInputProps } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { superficie } from '../../theme/superficie';

export function LoginField({ label, ...props }: TextInputProps & { label: string }) {
  const { theme } = useUnistyles();
  return <View style={styles.field}>
    <Text style={styles.label}>{label}</Text>
    <TextInput style={styles.input} placeholderTextColor={theme.tokens.text.muted} autoCapitalize="none" autoCorrect={false}
      accessibilityLabel={label} {...props} />
  </View>;
}

export function LoginSubmit({ label, text = label, blocked, onPress }: { label: string; text?: string; blocked: boolean; onPress: () => void }) {
  return <Pressable style={[styles.button, blocked && styles.buttonBlocked]} onPress={onPress} disabled={blocked}
    accessibilityRole="button" accessibilityLabel={label} accessibilityState={{ disabled: blocked }}>
    <Text style={[styles.buttonText, blocked && styles.buttonTextBlocked]}>{text}</Text>
  </Pressable>;
}

const styles = StyleSheet.create((theme) => ({
  field: {
    gap: theme.base.space[2],
  },
  label: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.medium,
    color: theme.tokens.text.secondary,
  },
  input: {
    height: 48,
    backgroundColor: superficie(theme, 0.8),
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    color: theme.tokens.text.primary,
    fontSize: theme.base.text.sm,
    paddingHorizontal: theme.base.space[4],
  },
  button: {
    height: 52,
    backgroundColor: theme.tokens.accent.base,
    borderRadius: theme.base.radius.md,
    justifyContent: 'center',
    alignItems: 'center',
    minHeight: 44,
  },
  buttonBlocked: {
    backgroundColor: theme.tokens.bg.hover,
  },
  buttonText: {
    color: theme.ink.onAccent,
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
  },
  buttonTextBlocked: {
    color: theme.tokens.text.muted,
  },
}));
