import { Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { Activity } from '@hangar/core';
import * as m from '../../paraglide/messages';

interface Props {
  tasks: Activity['tasks'];
}

function mark(status: string): string {
  if (status === 'completed') return '✓';
  if (status === 'in_progress') return '◐';
  return '○';
}

export function TaskRows({ tasks }: Props) {
  const { theme } = useUnistyles();
  if (!tasks.length) return null;
  return (
    <View style={styles.section}>
      <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.atividade_tarefas()}</Text>
      {tasks.map((t) => (
        <View
          key={t.id}
          style={styles.row}
          accessibilityRole="text"
          accessibilityLabel={`${mark(t.status)} ${t.title}`}
        >
          <Text
            style={[
              styles.mark,
              t.status === 'completed'
                ? { color: theme.tokens.status.success }
                : t.status === 'in_progress'
                  ? { color: theme.tokens.accent.base }
                  : { color: theme.tokens.text.muted },
            ]}
          >
            {mark(t.status)}
          </Text>
          <Text style={[styles.text, { color: t.status === 'in_progress' ? theme.tokens.text.primary : theme.tokens.text.secondary }]} numberOfLines={1}>
            {t.status === 'in_progress' && t.activeForm ? t.activeForm : t.title}
          </Text>
        </View>
      ))}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  section: {
    gap: theme.base.space[2],
  },
  label: {
    fontSize: theme.base.text.xs,
    textTransform: 'uppercase',
    letterSpacing: 0.6,
    fontWeight: '600',
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    minHeight: 28,
  },
  mark: {
    width: 20,
    textAlign: 'center',
    fontSize: theme.base.text.sm,
  },
  text: {
    flex: 1,
    fontSize: theme.base.text.sm,
    minWidth: 0,
  },
}));
