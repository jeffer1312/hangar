import type { ReactNode } from 'react';
import { Pressable, Text } from 'react-native';
import { Stack } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { StyleSheet } from 'react-native-unistyles';
import { Pagina } from '../config/Pagina';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

export function MachinePage({ title, children, save }: {
  title: string;
  children: ReactNode;
  save?: { onPress: () => void; disabled: boolean; busy?: boolean };
}) {
  const c = useSettingsColors();
  return <>
    <Stack.Screen options={{ title, headerRight: save ? () => <Pressable accessibilityRole="button"
      accessibilityLabel={m.mobile_machines_save()} accessibilityState={{ disabled: save.disabled, busy: save.busy }}
      disabled={save.disabled} onPress={save.onPress} style={styles.action}>
      <Text style={[styles.label, { color: c.accent, opacity: save.disabled ? 0.45 : 1 }]}>
        {save.busy ? m.mobile_machines_busy() : m.mobile_machines_save()}
      </Text>
    </Pressable> : undefined }} />
    <KeyboardAvoidingView automaticOffset behavior="padding" style={{ flex: 1 }}>
      <Pagina>{children}</Pagina>
    </KeyboardAvoidingView>
  </>;
}

const styles = StyleSheet.create((theme) => ({
  label: { fontSize: theme.base.text.base },
  action: { minWidth: 44, minHeight: 44, justifyContent: 'center', alignItems: 'center' },
}));
