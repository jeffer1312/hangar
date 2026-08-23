import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { SessionInfo, State } from '@hangar/core';
import { rotuloEstado } from '@hangar/core';
import * as m from '../../paraglide/messages';

interface Props {
  peers: string[];
  candidates: SessionInfo[];
  sessions: SessionInfo[];
  picked: string[];
  adding: boolean;
  busy: boolean;
  onOpenPeer: (name: string) => void;
  onToggleAdding: () => void;
  onToggle: (name: string) => void;
  onAdd: () => void;
  onLeave: () => void;
}

export function PairMembers({ peers, candidates, sessions, picked, adding, busy, onOpenPeer, onToggleAdding, onToggle, onAdd, onLeave }: Props) {
  const { theme } = useUnistyles();
  const stateOf = (name: string): State | null => sessions.find((session) => session.name === name)?.state ?? null;
  const stateDot = (state: State) => {
    if (state === 'working') return theme.tokens.accent.base;
    if (state === 'awaiting_input') return theme.tokens.status.warning;
    if (state === 'dead') return theme.tokens.status.error;
    return theme.tokens.status.success;
  };

  return (
    <View style={styles.content}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.par_grupo_titulo({ n: peers.length + 1 })}</Text>
      <Text style={[styles.hint, { color: theme.tokens.text.secondary }]}>{m.par_membros_hint()}</Text>

      <View style={styles.list}>
        {peers.map((peer) => {
          const state = stateOf(peer);
          return (
            <Pressable
              key={peer}
              onPress={() => onOpenPeer(peer)}
              style={styles.memberRow}
              accessibilityRole="button"
              accessibilityLabel={m.par_abrir_conversa_de({ nome: peer })}
            >
              <View style={[styles.dot, { backgroundColor: state ? stateDot(state) : theme.tokens.text.muted }]} />
              <View style={styles.main}>
                <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>{peer}</Text>
                {state ? <Text style={[styles.state, { color: theme.tokens.text.muted }]}>{rotuloEstado(state)}</Text> : null}
              </View>
              <Text style={[styles.chevron, { color: theme.tokens.text.muted }]}>›</Text>
            </Pressable>
          );
        })}
      </View>

      {!adding ? (
        <Pressable onPress={onToggleAdding} style={[styles.secondary, { borderColor: theme.tokens.border.default }]} accessibilityRole="button">
          <Text style={[styles.secondaryText, { color: theme.tokens.text.secondary }]}>{m.par_adicionar_sessao()}</Text>
        </Pressable>
      ) : (
        <View style={styles.addingBlock}>
          <View style={styles.list}>
            {candidates.length === 0 ? (
              <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.par_vazio_fora_grupo()}</Text>
            ) : (
              candidates.map((session) => {
                const state = session.state;
                const selected = picked.includes(session.name);
                return (
                  <Pressable
                    key={session.name}
                    onPress={() => onToggle(session.name)}
                    style={[styles.row, { borderColor: selected ? theme.tokens.accent.base : 'transparent', backgroundColor: selected ? theme.tokens.bg.elevated : 'transparent' }]}
                    accessibilityRole="checkbox"
                    accessibilityState={{ checked: selected }}
                    accessibilityLabel={m.par_adicionar_aria({ nome: session.name, estado: rotuloEstado(state) })}
                  >
                    <View style={[styles.dot, { backgroundColor: stateDot(state) }]} />
                    <View style={styles.main}>
                      <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>{session.name}</Text>
                      {session.cwd ? <Text style={[styles.cwd, { color: theme.tokens.text.muted }]} numberOfLines={1}>{session.cwd}</Text> : null}
                    </View>
                    {session.pair_peers?.length ? <Text style={[styles.paired, { color: theme.tokens.text.muted }]}>{`🤝 ${session.pair_peers.length}`}</Text> : null}
                  </Pressable>
                );
              })
            )}
          </View>
          <Pressable onPress={onAdd} disabled={!picked.length || busy} style={[styles.primary, { backgroundColor: theme.tokens.accent.base }, (!picked.length || busy) && styles.disabled]} accessibilityRole="button">
            <Text style={[styles.primaryText, { color: theme.tokens.text.inverse }]}>
              {busy ? m.par_adicionando() : picked.length ? m.par_adicionar_nomes({ nomes: picked.join(', ') }) : m.par_escolha_sessoes()}
            </Text>
          </Pressable>
        </View>
      )}

      <Pressable onPress={onLeave} disabled={busy} style={[styles.danger, { borderColor: theme.tokens.status.error }, busy && styles.disabled]} accessibilityRole="button">
        <Text style={[styles.dangerText, { color: theme.tokens.status.error }]}>{busy ? m.par_saindo() : m.par_sair_grupo()}</Text>
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  content: { gap: theme.base.space[3] },
  title: { fontSize: theme.base.text.lg, fontWeight: '700' },
  hint: { fontSize: theme.base.text.sm, lineHeight: 21 },
  list: { gap: theme.base.space[1] },
  memberRow: { minHeight: 56, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderRadius: theme.base.radius.md },
  row: { minHeight: 56, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderWidth: 1, borderRadius: theme.base.radius.md },
  dot: { width: 8, height: 8, borderRadius: 4, flexShrink: 0 },
  main: { flex: 1, minWidth: 0, gap: 2 },
  name: { fontSize: theme.base.text.base, fontWeight: '600' },
  state: { fontSize: theme.base.text.xs },
  cwd: { fontSize: theme.base.text.xs },
  paired: { fontSize: theme.base.text.xs, flexShrink: 0 },
  chevron: { fontSize: 24, lineHeight: 24 },
  addingBlock: { gap: theme.base.space[3] },
  empty: { fontSize: theme.base.text.sm, textAlign: 'center', paddingVertical: theme.base.space[3] },
  secondary: { minHeight: 44, borderWidth: 1, borderStyle: 'dashed', borderRadius: theme.base.radius.md, alignItems: 'center', justifyContent: 'center', paddingHorizontal: theme.base.space[3] },
  secondaryText: { fontSize: theme.base.text.sm, fontWeight: '600' },
  primary: { minHeight: 50, borderRadius: theme.base.radius.md, alignItems: 'center', justifyContent: 'center', paddingHorizontal: theme.base.space[3] },
  primaryText: { fontSize: theme.base.text.base, fontWeight: '700', textAlign: 'center' },
  danger: { minHeight: 50, borderWidth: 1, borderRadius: theme.base.radius.md, alignItems: 'center', justifyContent: 'center', paddingHorizontal: theme.base.space[3] },
  dangerText: { fontSize: theme.base.text.base, fontWeight: '700' },
  disabled: { opacity: 0.45 },
}));
