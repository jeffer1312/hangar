import { useEffect, useRef, useState } from 'react';
import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { terminateCodexCommand, type CodexCommand, type Server } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { chatStore } from '../../stores/chat';
import * as m from '../../paraglide/messages';

export function CodexCommands({ commands, name, serverId, server, readOnly = false }: {
  commands: CodexCommand[]; name: string; serverId: string; server: Server | null; readOnly?: boolean;
}) {
  const { theme } = useUnistyles();
  const [opened, setOpened] = useState<string | null>(null);
  const [pending, setPending] = useState<string[]>([]);
  const pendingIds = useRef(new Set<string>());
  const [error, setError] = useState('');
  const output = chatStore(serverId, name).use(state => opened ? state.commandOutput[opened] ?? '' : '');
  const sequence = useRef(0);
  useEffect(() => () => { sequence.current += 1; }, []);
  useEffect(() => {
    if (pending.some(id => !commands.some(command => command.id === id))) {
      for (const id of pendingIds.current) {
        if (!commands.some(command => command.id === id)) pendingIds.current.delete(id);
      }
      setPending([...pendingIds.current]);
    }
    if (opened && !commands.some(command => command.id === opened)) setOpened(null);
  }, [commands, pending, opened]);

  async function stop(command: CodexCommand) {
    if (!server || pendingIds.current.has(command.id) || readOnly) return;
    const request = sequence.current;
    pendingIds.current.add(command.id);
    setPending([...pendingIds.current]);
    setError('');
    try {
      await terminateCodexCommand(name, command.process_id, server);
    } catch (error) {
      if (sequence.current !== request || !pendingIds.current.has(command.id)) return;
      setError(error instanceof Error ? error.message : m.codex_command_stop_failed());
    } finally {
      if (sequence.current === request) {
        pendingIds.current.delete(command.id);
        setPending([...pendingIds.current]);
      }
    }
  }

  if (!commands.length) return null;
  return <View style={styles.section}>
    <Text style={styles.title}>{m.codex_commands_title()}</Text>
    {commands.map(command => <View key={command.id} style={styles.command}>
      <View style={styles.row}>
        <Pressable onPress={() => setOpened(opened === command.id ? null : command.id)} style={styles.open}
          accessibilityRole="button" accessibilityState={{ expanded: opened === command.id }}
          accessibilityLabel={`${m.codex_command_output()}: ${command.command}`}>
          <Icon name={opened === command.id ? 'ChevronDown' : 'ChevronRight'} size={18} color={theme.tokens.text.muted} />
          <View style={styles.body}>
            <Text style={styles.text} numberOfLines={2}>{command.command}</Text>
            <Text style={styles.meta}>pid {command.process_id} · {command.background ? m.codex_terminals_count({ n: 1 }) : m.native_tool_running()}</Text>
          </View>
        </Pressable>
        {!readOnly ? <Pressable onPress={() => void stop(command)} disabled={!server || pending.includes(command.id)} style={styles.stop}
          accessibilityRole="button" accessibilityState={{ disabled: !server || pending.includes(command.id) }}
          accessibilityLabel={m.codex_command_stop_label({ command: command.command })}>
          <Text style={styles.action}>{pending.includes(command.id) ? m.codex_command_stopping() : m.codex_command_stop()}</Text>
        </Pressable> : null}
      </View>
      {opened === command.id ? <ScrollView style={styles.output} nestedScrollEnabled>
        <Text selectable style={styles.text}>{output || m.codex_command_no_output()}</Text>
      </ScrollView> : null}
    </View>)}
    {error ? <Text accessibilityRole="alert" style={styles.error}>{error}</Text> : null}
  </View>;
}

const styles = StyleSheet.create(theme => ({
  section: { gap: theme.base.space[2] },
  title: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  command: { gap: theme.base.space[1] },
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  open: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 44 },
  body: { flex: 1, gap: theme.base.space[1] },
  text: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.primary },
  meta: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  stop: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[2] },
  action: { fontSize: theme.base.text.sm, color: theme.tokens.accent.base },
  output: { maxHeight: 240, padding: theme.base.space[2] },
  error: { fontSize: theme.base.text.sm, color: theme.tokens.status.error },
}));
