import { useState } from 'react';
import { Pressable, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { contextRefLabel } from '@hangar/core';
import * as m from '../paraglide/messages';
import { Icon } from './Icon';
import { superficie } from '../theme/superficie';
import { removeContextRef, setContextRefComment, type DraftContextRef } from '../stores/contextRefs';

// Bandeja de citações acima do campo de mensagem: mesmo cartão da prévia do anexo, uma linha por
// citação. O lápis abre o comentário; o X tira da bandeja.
export function ContextRefTray({ refs, draftKey, disabled }: { refs: DraftContextRef[]; draftKey: string; disabled?: boolean }) {
  const { theme } = useUnistyles();
  const [editing, setEditing] = useState<string | null>(null);
  return (
    <View style={styles.tray} accessibilityLabel={m.cite_tray_aria()}>
      {refs.map((ref, i) => {
        const label = contextRefLabel(ref);
        return (
          <View key={ref.draftId} style={[styles.item, { backgroundColor: superficie(theme, 0.8), borderColor: theme.tokens.border.subtle }]}>
            <View style={styles.row}>
              <Icon name={ref.kind === 'quote' ? 'TextQuote' : ref.kind === 'code' ? 'FileCode' : 'MousePointerClick'} size={16} color={theme.tokens.accent.base} />
              <Text style={[styles.label, { color: theme.tokens.text.primary }]} numberOfLines={1}>{label}</Text>
              <Pressable
                onPress={() => setEditing(editing === ref.draftId ? null : ref.draftId)}
                style={[styles.btn, { borderColor: theme.tokens.border.subtle }]}
                accessibilityRole="button"
                accessibilityLabel={m.cite_edit_comment()}
                accessibilityState={{ expanded: editing === ref.draftId }}
              >
                <Icon name="Pencil" size={16} color={theme.tokens.text.secondary} />
              </Pressable>
              <Pressable
                onPress={() => { if (editing === ref.draftId) setEditing(null); removeContextRef(draftKey, i); }}
                disabled={disabled}
                style={[styles.btn, { borderColor: theme.tokens.border.subtle }, disabled && styles.disabled]}
                accessibilityRole="button"
                accessibilityLabel={m.cite_remove()}
                accessibilityState={{ disabled: !!disabled }}
              >
                <Icon name="X" size={16} color={theme.tokens.text.secondary} />
              </Pressable>
            </View>
            {editing === ref.draftId ? (
              <TextInput
                autoFocus
                value={ref.comment}
                // Durante o envio o comentário já foi junto: editar agora seria apagado com a citação.
                editable={!disabled}
                onChangeText={(v) => setContextRefComment(draftKey, ref.draftId, v)}
                onSubmitEditing={() => setEditing(null)}
                placeholder={m.cite_comment_placeholder()}
                placeholderTextColor={theme.tokens.text.muted}
                accessibilityLabel={m.cite_comment_aria()}
                returnKeyType="done"
                style={[styles.input, { color: theme.tokens.text.primary, borderColor: theme.tokens.border.subtle }]}
              />
            ) : null}
          </View>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  tray: { gap: theme.base.space[1] },
  item: {
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    paddingVertical: theme.base.space[1],
    paddingLeft: theme.base.space[2],
    paddingRight: theme.base.space[1],
    gap: theme.base.space[1],
  },
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  label: { flex: 1, fontSize: theme.base.text.sm },
  btn: { width: 40, height: 40, borderRadius: theme.base.radius.full, borderWidth: 1, alignItems: 'center', justifyContent: 'center' },
  disabled: { opacity: 0.5 },
  input: {
    borderWidth: 1,
    borderRadius: theme.base.radius.sm,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[2],
    fontSize: theme.base.text.base,
    marginRight: theme.base.space[1],
    marginBottom: theme.base.space[1],
  },
}));
