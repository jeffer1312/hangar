// "Consertar…" do aviso de hook: o que a sessão de conserto vai receber, antes de abri-la.
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Sheet } from '../ui/Sheet';
import type { FixPreview } from '../features/notices/api';
import * as m from '../paraglide/messages';

/** `preview` fica depois de fechar: o conteúdo não some antes de a folha descer. */
export function NoticeFixSheet({ open, preview, busy, error, onConfirm, onClose }: {
  open: boolean; preview: FixPreview | null; busy: boolean; error: string | null; onConfirm: () => void; onClose: () => void;
}) {
  const { theme } = useUnistyles();
  return (
    <Sheet open={open} sizes={['auto']} onDismiss={onClose}>
      {preview ? (
        <View style={styles.body}>
          <Text style={styles.title}>{m.notice_fix_title({ name: preview.subject })}</Text>
          <Text style={styles.intro}>{m.notice_fix_intro()}</Text>
          <Text style={styles.label}>{m.notice_fix_error_label()}</Text>
          <Text style={styles.mono} selectable>{preview.detail}</Text>
          <Text style={styles.label}>{m.notice_fix_files_label()}</Text>
          <Text style={styles.mono} selectable>{preview.files.length ? preview.files.join('\n') : m.notice_fix_no_files()}</Text>
          <Text style={styles.footer}>{preview.no_hooks ? m.notice_fix_footer_no_hooks() : m.notice_fix_footer()}</Text>
          {error ? <Text style={styles.error}>{m.notice_fix_start_failed({ why: error })}</Text> : null}
          <Pressable style={styles.primary} onPress={onConfirm} disabled={busy} accessibilityRole="button">
            {busy ? <ActivityIndicator color={theme.ink.onAccent} /> : <Text style={styles.primaryText}>{m.notice_fix_open()}</Text>}
          </Pressable>
          <Pressable style={styles.secondary} onPress={onClose} accessibilityRole="button">
            <Text style={styles.secondaryText}>{m.comum_cancelar()}</Text>
          </Pressable>
        </View>
      ) : null}
    </Sheet>
  );
}

const styles = StyleSheet.create((theme) => ({
  body: { paddingHorizontal: theme.base.space[5], paddingTop: theme.base.space[4], paddingBottom: theme.base.space[6], gap: theme.base.space[2] },
  title: { ...theme.type.heading, color: theme.tokens.text.primary },
  intro: { ...theme.type.body, color: theme.tokens.text.secondary },
  label: { ...theme.type.caption, color: theme.tokens.text.muted, textTransform: 'uppercase', marginTop: theme.base.space[2] },
  mono: {
    ...theme.type.mono,
    color: theme.tokens.text.secondary,
    backgroundColor: theme.tokens.bg.hover,
    borderRadius: theme.base.radius.sm,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
  },
  footer: { ...theme.type.caption, color: theme.tokens.text.muted, marginTop: theme.base.space[2] },
  error: { ...theme.type.caption, color: theme.tokens.status.warning },
  primary: {
    minHeight: 50,
    borderRadius: theme.base.radius.md,
    backgroundColor: theme.tokens.accent.base,
    alignItems: 'center',
    justifyContent: 'center',
    marginTop: theme.base.space[3],
  },
  primaryText: { ...theme.type.control, color: theme.ink.onAccent },
  secondary: { minHeight: 44, alignItems: 'center', justifyContent: 'center' },
  secondaryText: { ...theme.type.label, color: theme.tokens.accent.base },
}));
