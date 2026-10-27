import { useRef, useState } from 'react';
import { Pressable, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { formataErro, unlockKeyringForServer, type Server } from '@hangar/core';
import { haptic } from '../../ui/haptics';
import { Icon } from '../../ui/Icon';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';

// O aviso some sozinho quando o evento `keyring` da lista chega destrancado.
export function KeyringNotice({ server }: { server: Server }) {
  const { theme } = useUnistyles();
  const [password, setPassword] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState('');
  const [visible, setVisible] = useState(false);
  const field = useRef<TextInput>(null);

  const unlock = async () => {
    if (sending || !password) return;
    const typed = password;
    setPassword('');
    setSending(true);
    setError('');
    try {
      await unlockKeyringForServer(server, typed);
      haptic.success();
    } catch (e) {
      setError((e as { code?: unknown } | null)?.code === 'keyring_wrong_password'
        ? m.keyring_wrong_password()
        : m.keyring_unlock_failed({ error: formataErro(e) ?? String(e) }));
      haptic.error();
      // Recusada: o campo volta vazio e com o foco, para digitar de novo.
      requestAnimationFrame(() => field.current?.focus());
    } finally {
      setSending(false);
      setVisible(false);
    }
  };

  const disabled = sending || !password;
  return (
    <View style={styles.box}>
      <View style={styles.head}>
        <Icon name="Lock" size={18} color={theme.tokens.status.warning} />
        <View style={styles.headText}>
          <Text style={[styles.title, { color: theme.tokens.text.primary }]} accessibilityRole="alert">
            {m.keyring_locked_title({ label: server.label })}
          </Text>
          <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{m.keyring_locked_hint()}</Text>
        </View>
      </View>
      <View style={[styles.field, { backgroundColor: superficie(theme, 0.8), borderColor: error ? theme.tokens.status.error : theme.tokens.border.subtle }]}>
        <TextInput
          ref={field}
          value={password}
          onChangeText={(text) => { setPassword(text); if (error) setError(''); }}
          secureTextEntry={!visible}
          autoComplete="off"
          textContentType="none"
          autoCapitalize="none"
          autoCorrect={false}
          editable={!sending}
          placeholder={m.keyring_password_placeholder()}
          accessibilityLabel={m.keyring_password_placeholder()}
          placeholderTextColor={theme.tokens.text.muted}
          returnKeyType="go"
          onSubmitEditing={unlock}
          style={[styles.input, { color: theme.tokens.text.primary }]}
        />
        <Pressable
          onPress={() => setVisible((v) => !v)}
          disabled={sending}
          hitSlop={4}
          accessibilityRole="button"
          accessibilityLabel={visible ? m.keyring_hide_password() : m.keyring_show_password()}
          accessibilityState={{ selected: visible }}
          style={styles.reveal}
        >
          <Icon name={visible ? 'EyeOff' : 'Eye'} size={20} color={theme.tokens.text.muted} />
        </Pressable>
      </View>
      {error ? <Text style={[styles.hint, { color: theme.tokens.status.error }]} accessibilityLiveRegion="polite" selectable>{error}</Text> : null}
      <Pressable
        onPress={unlock}
        disabled={disabled}
        accessibilityRole="button"
        accessibilityState={{ disabled, busy: sending }}
        style={[styles.button, { backgroundColor: disabled ? superficie(theme, 0.8) : theme.tokens.accent.base }]}
      >
        <Text style={[styles.buttonText, disabled && { color: theme.tokens.text.muted }]}>
          {sending ? m.keyring_unlocking() : m.keyring_unlock()}
        </Text>
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  box: { paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], gap: theme.base.space[2] },
  head: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[2] },
  headText: { flex: 1, gap: 2 },
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  hint: { fontSize: theme.base.text.xs },
  field: { flexDirection: 'row', alignItems: 'center', borderWidth: 1, borderRadius: theme.base.radius.lg },
  input: { flex: 1, minHeight: 44, paddingHorizontal: theme.base.space[3], fontSize: theme.base.text.base },
  reveal: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  button: { minHeight: 44, borderRadius: theme.base.radius.full, alignItems: 'center', justifyContent: 'center' },
  buttonText: { color: theme.ink.onAccent, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
}));
