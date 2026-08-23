import { ActivityIndicator, Modal, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Glass } from '../../ui/Glass';
import * as m from '../../paraglide/messages';

export interface PillMenuItem {
  label: string;
  hint?: string;
  selected?: boolean;
}

interface Props {
  open: boolean;
  onClose: () => void;
  items: PillMenuItem[];
  loading?: boolean;
  error?: string | null;
  onRetry?: () => void;
  onSelect: (item: PillMenuItem) => void;
  title?: string;
}

export function PillMenu({ open, onClose, items, loading, error, onRetry, onSelect, title }: Props) {
  const { theme } = useUnistyles();
  if (!open) return null;
  return (
    <Modal transparent visible={open} animationType="fade" onRequestClose={onClose}>
      <Pressable style={styles.backdrop} onPress={onClose}>
        <Pressable onPress={(e) => e.stopPropagation()} style={styles.sheetWrap}>
          <Glass variant="modal" style={styles.sheet}>
            {title ? <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{title}</Text> : null}
            {loading ? (
              <View style={styles.center}>
                <ActivityIndicator color={theme.tokens.text.secondary} />
                <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
              </View>
            ) : error ? (
              <View style={styles.center}>
                <Text style={[styles.err, { color: theme.tokens.status.error }]}>{error}</Text>
                {onRetry ? (
                  <Pressable onPress={onRetry} style={[styles.retryBtn, { borderColor: theme.tokens.border.subtle }]}>
                    <Text style={[styles.retryText, { color: theme.tokens.accent.base }]}>{m.lista_tentar_novamente()}</Text>
                  </Pressable>
                ) : null}
              </View>
            ) : items.length === 0 ? (
              <View style={styles.center}>
                <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_nenhum_modelo()}</Text>
              </View>
            ) : (
              <ScrollView style={styles.list} contentContainerStyle={styles.listContent}>
                {items.map((it) => (
                  <Pressable
                    key={it.label + (it.hint ?? '')}
                    onPress={() => onSelect(it)}
                    style={[styles.row, it.selected && { backgroundColor: theme.tokens.bg.elevated }]}
                    accessibilityRole="button"
                    accessibilityState={{ selected: !!it.selected }}
                  >
                    <View style={styles.rowText}>
                      <Text style={[styles.label, { color: theme.tokens.text.primary }]} numberOfLines={1}>
                        {it.label}
                      </Text>
                      {it.hint ? (
                        <Text style={[styles.hint, { color: theme.tokens.text.muted }]} numberOfLines={1}>
                          {it.hint}
                        </Text>
                      ) : null}
                    </View>
                    {it.selected ? (
                      <Text style={[styles.tick, { color: theme.tokens.accent.base }]}>✓</Text>
                    ) : null}
                  </Pressable>
                ))}
              </ScrollView>
            )}
            <Pressable onPress={onClose} style={[styles.closeBtn, { borderColor: theme.tokens.border.subtle }]}>
              <Text style={[styles.closeText, { color: theme.tokens.text.secondary }]}>{m.sessao_fechar()}</Text>
            </Pressable>
          </Glass>
        </Pressable>
      </Pressable>
    </Modal>
  );
}

const styles = StyleSheet.create((theme) => ({
  backdrop: {
    flex: 1,
    backgroundColor: 'rgba(0,0,0,0.4)',
    justifyContent: 'flex-end',
  },
  sheetWrap: {
    maxHeight: '70%',
  },
  sheet: {
    margin: theme.base.space[2],
    padding: theme.base.space[2],
    gap: theme.base.space[2],
  },
  title: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
    textAlign: 'center',
  },
  center: {
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingVertical: theme.base.space[4],
  },
  muted: {
    fontSize: theme.base.text.sm,
    textAlign: 'center',
  },
  err: {
    fontSize: theme.base.text.sm,
    textAlign: 'center',
  },
  retryBtn: {
    marginTop: theme.base.space[1],
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
  },
  retryText: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
  list: {
    maxHeight: 320,
  },
  listContent: {
    gap: 2,
  },
  row: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.md,
    gap: theme.base.space[2],
  },
  rowText: {
    flex: 1,
    gap: 2,
  },
  label: {
    fontSize: theme.base.text.sm,
    fontWeight: '500',
  },
  hint: {
    fontSize: theme.base.text.xs,
  },
  tick: {
    fontSize: 16,
    fontWeight: '700',
  },
  closeBtn: {
    marginTop: theme.base.space[1],
    alignSelf: 'center',
    paddingHorizontal: theme.base.space[4],
    paddingVertical: theme.base.space[2],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
  },
  closeText: {
    fontSize: theme.base.text.sm,
  },
}));
