import { Pressable, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { SessionInfo, State } from '@hangar/core';
import { rotuloEstado } from '@hangar/core';
import * as m from '../../paraglide/messages';

interface Props {
  sessions: SessionInfo[];
  picked: string[];
  task: string;
  busy: boolean;
  error: string;
  loading: boolean;
  onToggle: (name: string) => void;
  onTaskChange: (value: string) => void;
  onPair: () => void;
}

export function PairPicker({ sessions, picked, task, busy, error, loading, onToggle, onTaskChange, onPair }: Props) {
  const { theme } = useUnistyles();
  const stateDot = (state: State) => {
    if (state === 'working') return theme.tokens.accent.base;
    if (state === 'awaiting_input') return theme.tokens.status.warning;
    if (state === 'dead') return theme.tokens.status.error;
    return theme.tokens.status.success;
  };

  return (
    <View style={styles.content}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.par_parear_titulo()}</Text>
      <Text style={[styles.hint, { color: theme.tokens.text.secondary }]}>{m.par_passam_hint()}</Text>

      {error ? <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert" selectable>{error}</Text> : null}
      {loading && sessions.length === 0 ? <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text> : null}

      <View style={styles.list}>
        {!loading && sessions.length === 0 && !error ? (
          <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.forward_nenhuma_viva()}</Text>
        ) : (
          sessions.map((session) => {
            const selected = picked.includes(session.name);
            return (
              <Pressable
                key={session.name}
                onPress={() => onToggle(session.name)}
                style={[styles.row, { borderColor: selected ? theme.tokens.accent.base : 'transparent', backgroundColor: selected ? theme.tokens.bg.elevated : 'transparent' }]}
                accessibilityRole="checkbox"
                accessibilityState={{ checked: selected }}
                accessibilityLabel={m.par_parear_aria({ nome: session.name, estado: rotuloEstado(session.state) })}
              >
                <View style={[styles.dot, { backgroundColor: stateDot(session.state) }]} />
                <View style={styles.main}>
                  <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>{session.name}</Text>
                  {session.cwd ? <Text style={[styles.cwd, { color: theme.tokens.text.muted }]} numberOfLines={1}>{session.cwd}</Text> : null}
                </View>
                {session.pair_peers?.length ? (
                  <Text style={[styles.paired, { color: theme.tokens.text.muted }]}>{`🤝 ${session.pair_peers.length}`}</Text>
                ) : null}
              </Pressable>
            );
          })
        )}
      </View>

      <TextInput
        value={task}
        onChangeText={onTaskChange}
        placeholder={m.par_tarefa_placeholder()}
        placeholderTextColor={theme.tokens.text.muted}
        style={[styles.taskInput, { color: theme.tokens.text.primary, backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.default }]}
        returnKeyType="done"
        accessibilityLabel={m.par_tarefa_placeholder()}
      />

      <Pressable
        onPress={onPair}
        disabled={!picked.length || busy}
        style={[styles.primary, { backgroundColor: theme.tokens.accent.base }, (!picked.length || busy) && styles.disabled]}
        accessibilityRole="button"
      >
        <Text style={[styles.primaryText, { color: theme.tokens.text.inverse }]}>
          {busy ? m.par_pareando() : picked.length ? m.par_parear_nomes({ nomes: picked.join(', ') }) : m.par_escolha_varias()}
        </Text>
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  content: { gap: theme.base.space[3] },
  title: { fontSize: theme.base.text.lg, fontWeight: '700' },
  hint: { fontSize: theme.base.text.sm, lineHeight: 21 },
  error: { fontSize: theme.base.text.sm, lineHeight: 20 },
  empty: { fontSize: theme.base.text.sm, textAlign: 'center', paddingVertical: theme.base.space[3] },
  list: { gap: theme.base.space[1] },
  row: { minHeight: 56, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderWidth: 1, borderRadius: theme.base.radius.md },
  dot: { width: 8, height: 8, borderRadius: 4, flexShrink: 0 },
  main: { flex: 1, minWidth: 0, gap: 2 },
  name: { fontSize: theme.base.text.base, fontWeight: '600' },
  cwd: { fontSize: theme.base.text.xs },
  paired: { fontSize: theme.base.text.xs, flexShrink: 0 },
  taskInput: { minHeight: 48, borderWidth: 1, borderRadius: theme.base.radius.md, paddingHorizontal: theme.base.space[3], fontSize: 16 },
  primary: { minHeight: 50, borderRadius: theme.base.radius.md, alignItems: 'center', justifyContent: 'center', paddingHorizontal: theme.base.space[3] },
  primaryText: { fontSize: theme.base.text.base, fontWeight: '700', textAlign: 'center' },
  disabled: { opacity: 0.45 },
}));
