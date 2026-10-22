import { useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, ScrollView, Text, TextInput, View, type TextInputProps } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { ReactNode } from 'react';
import { Sheet } from '../../ui/Sheet';
import { Icon } from '../../ui/Icon';
import { useSettingsColors } from '../config/colors';
import { ensureActive, reason, type Directions } from './api';
import * as m from '../../paraglide/messages';

export function MachineSheet({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const insets = useSafeAreaInsets();
  const c = useSettingsColors();
  return <Sheet open sizes={['large']} scrollable onDismiss={onClose}>
    <KeyboardAvoidingView automaticOffset behavior="padding" style={{ flex: 1 }}>
      <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={[styles.sheet, { paddingBottom: insets.bottom + 24 }]}>
        <View style={styles.heading}>
          <Text accessibilityRole="header" style={[styles.title, { color: c.text }]}>{title}</Text>
          <Pressable accessibilityRole="button" accessibilityLabel={m.sessao_fechar()} onPress={onClose} style={styles.close}>
            <Icon name="X" size={21} color={c.muted} />
          </Pressable>
        </View>
        {children}
      </ScrollView>
    </KeyboardAvoidingView>
  </Sheet>;
}
export function ConnectionDot({ label, loaded, error }: { label: string; loaded: boolean; error?: string | null }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const status = error ? error === 'offline' ? m.mobile_machines_offline() : error : loaded ? m.mobile_machines_connected() : '';
  return <View accessible accessibilityLabel={status ? `${label}, ${status}` : label}
    style={{ width: 8, height: 8, borderRadius: theme.base.radius.full, backgroundColor: error ? theme.tokens.status.error : loaded ? theme.tokens.status.success : c.faint }} />;
}

export function Field({ label, ...props }: TextInputProps & { label: string }) {
  const c = useSettingsColors();
  return <View style={styles.field}>
    <Text style={[styles.fieldLabel, { color: c.text }]}>{label}</Text>
    <TextInput autoCapitalize="none" autoCorrect={false} {...props} accessibilityLabel={label}
      placeholderTextColor={c.faint} style={[styles.input, { color: c.text, borderColor: c.borderStrong, backgroundColor: c.inset }]} />
  </View>;
}
export function Notice({ text, error = false }: { text: string; error?: boolean }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return text ? <Text selectable accessibilityRole={error ? 'alert' : undefined} accessibilityLiveRegion="polite"
    style={[styles.notice, { color: error ? theme.tokens.status.error : c.text }]}>{text}</Text> : null;
}

export function DirectionsNotice({ value }: { value: Directions }) {
  return <View style={styles.field}>{(['forward', 'back'] as const).map((direction) => {
    const check = value[direction];
    const error = direction === 'forward' ? value.forwardError : value.backError;
    const title = direction === 'forward' ? m.mobile_machines_forward() : m.mobile_machines_back();
    const text = check ? `${check.estado} · ${check.identificador || '—'}${check.tempo_ms === null ? '' : ` · ${check.tempo_ms} ms`}${check.motivo ? ` · ${check.motivo}` : ''}${error ? ` · ${error}` : ''}` : error || '—';
    return <Notice key={direction} text={`${title}: ${text}`} error={!!error || (!!check && check.estado !== 'ok')} />;
  })}</View>;
}

// Cada abertura tem seu sinal: resposta antiga não pode salvar nem pintar a máquina seguinte.
export function useMachineAction() {
  const controller = useRef(new AbortController());
  const locked = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  useEffect(() => {
    if (controller.current.signal.aborted) {
      controller.current = new AbortController();
      locked.current = false;
    }
    const current = controller.current;
    return () => current.abort();
  }, []);
  const run = useCallback(async <T,>(work: (signal: AbortSignal) => Promise<T>, commit: (value: T) => void, done = '') => {
    const signal = controller.current.signal;
    if (locked.current || signal.aborted) return;
    locked.current = true; setBusy(true); setError(''); setMessage('');
    try {
      const value = await work(signal);
      ensureActive(signal);
      commit(value); setMessage(done);
    } catch (e) { if (!signal.aborted) setError(reason(e)); }
    finally { if (!signal.aborted) { locked.current = false; setBusy(false); } }
  }, []);
  return { busy, error, message, run };
}
export const styles = StyleSheet.create((theme) => ({
  sheet: { padding: theme.base.space[4], gap: theme.base.space[4] },
  heading: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: theme.base.space[3] },
  title: { flex: 1, fontSize: theme.base.text.xl, fontWeight: theme.base.weight.semibold },
  close: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  fieldLabel: { fontSize: theme.base.text.sm },
  notice: { fontSize: theme.base.text.xs, lineHeight: 19 },
  field: { gap: theme.base.space[2], paddingVertical: theme.base.space[2] },
  input: { minHeight: 44, paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderWidth: 1, borderRadius: theme.base.radius.sm, fontSize: theme.base.text.sm },
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2], paddingVertical: theme.base.space[2] },
  content: { padding: theme.base.space[4], gap: theme.base.space[3] },
}));
