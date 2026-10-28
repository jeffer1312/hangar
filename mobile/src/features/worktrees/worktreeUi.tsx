import { Pressable, Text, View } from 'react-native';
import { router } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { fmtBytes, worktreeIsAgent, worktreesSizeTotal, worktreeTitle, type State, type WorktreeState, type WorktreeStatus } from '@hangar/core';
import type { Tone } from '../../ui/Chip';
import { StateDot } from '../../ui/StateDot';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';

export const STATE_TONE: Record<WorktreeState, Tone> = {
  gone: 'error', session: 'accent', residual: 'warning', dirty: 'warning', merged: 'success', detached: 'neutral', active: 'neutral',
};

export function stateLabel(k: WorktreeState): string {
  switch (k) {
    case 'gone': return m.worktree_estado_sumida();
    case 'session': return m.worktree_estado_em_uso();
    case 'residual': return m.worktree_estado_residual();
    case 'dirty': return m.worktree_estado_nao_commitado();
    case 'merged': return m.worktree_estado_mesclada();
    case 'detached': return m.worktree_estado_sem_branch();
    case 'active': return m.worktree_estado_andamento();
  }
}

/** Cor do estado no tema do app: a mesma tinta do chip, para a barra do disco e a legenda. */
export function useStateColor() {
  const { theme } = useUnistyles();
  return (k: WorktreeState) => ({
    gone: theme.tokens.status.error,
    session: theme.tokens.accent.base,
    residual: theme.tokens.status.warning,
    dirty: theme.tokens.status.warning,
    merged: theme.tokens.status.success,
    detached: theme.tokens.text.muted,
    active: theme.tokens.text.secondary,
  })[k];
}

/** Pasta de subagente sem branch que diga algo: "Subagente ab1a". */
export function displayTitle(w: WorktreeStatus): string {
  const t = worktreeTitle(w);
  const pasta = w.path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? '';
  return worktreeIsAgent(w) && t === pasta ? m.worktree_subagente_nome({ id: pasta.slice(6, 10) }) : t;
}

export function sizeLabel(w: WorktreeStatus): string {
  if (!w.exists) return '—';
  if (w.size == null) {
    if (w.size_error) return m.worktree_tamanho_falhou();
    return w.size_pending ? m.worktrees_disco_calculando() : '—';
  }
  return w.size_partial ? m.worktree_tamanho_parcial({ tamanho: fmtBytes(w.size) }) : fmtBytes(w.size);
}

/** Soma de várias: com alguma medida pendente ou incompleta, o total aparece como mínimo ("≥"). */
export function sizeSumLabel(ws: WorktreeStatus[]): string {
  const { bytes, partial } = worktreesSizeTotal(ws);
  return partial ? m.worktree_tamanho_parcial({ tamanho: fmtBytes(bytes) }) : fmtBytes(bytes);
}

export function sessionStateLabel(s: State | undefined): string {
  if (s === 'working') return m.worktree_sessao_trabalhando();
  if (s === 'awaiting_input') return m.worktree_sessao_esperando();
  return m.worktree_sessao_parada();
}

/** Estado ao vivo vem da lista de sessões que o app já assina: nenhuma leitura por linha. */
export function useLiveSessionState(serverId: string, name: string): State | undefined {
  return useSessions((s) => s.byServerRecord?.[serverId]?.find((r) => r.name === name)?.state);
}

export function openSession(serverId: string, name: string) {
  router.push(`/s/${encodeURIComponent(serverId)}/${encodeURIComponent(name)}` as never);
}

function SessionChip({ serverId, name, onBeforeOpen }: { serverId: string; name: string; onBeforeOpen?: () => void }) {
  const { theme } = useUnistyles();
  const state = useLiveSessionState(serverId, name);
  return (
    <Pressable
      onPress={() => { onBeforeOpen?.(); openSession(serverId, name); }}
      accessibilityRole="button"
      accessibilityLabel={m.worktree_ir_sessao_nome({ nome: name })}
      hitSlop={6}
      style={({ pressed }) => [styles.chip, { borderColor: theme.tokens.border.default }, pressed && { opacity: 0.6 }]}
    >
      <StateDot state={state ?? 'idle'} size={7} />
      <Text style={[styles.chipName, { color: theme.tokens.text.primary }]} numberOfLines={1}>{name}</Text>
      <Text style={[styles.chipState, { color: theme.tokens.text.secondary }]}>{sessionStateLabel(state)}</Text>
    </Pressable>
  );
}

export function SessionChips({ serverId, names, onBeforeOpen }: { serverId: string; names: string[]; onBeforeOpen?: () => void }) {
  if (!names.length) return null;
  return (
    <View style={styles.chips}>
      {names.map((n) => <SessionChip key={n} serverId={serverId} name={n} onBeforeOpen={onBeforeOpen} />)}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[3] },
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], minHeight: 32, maxWidth: '100%',
    paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.full, borderWidth: 1,
  },
  chipName: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, flexShrink: 1 },
  chipState: { fontSize: theme.base.text.xxs },
}));
