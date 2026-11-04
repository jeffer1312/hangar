import { fmtBytes, worktreesSizeTotal, type State, type WorktreeRepo, type WorktreeState, type WorktreeStatus } from '@hangar/core';
import * as m from '../paraglide/messages';

// Tamanho de uma worktree: medição falha ou parcial aparece como tal, nunca como número cheio.
export function worktreeSizeLabel(w: WorktreeStatus): string {
  if (!w.exists) return '—';
  if (w.size_pending) return m.worktrees_disco_calculando();
  if (w.size_error) return m.worktree_tamanho_falhou();
  if (w.size == null) return '—';
  return w.size_partial ? m.worktree_tamanho_parcial({ tamanho: fmtBytes(w.size) }) : fmtBytes(w.size);
}

// Releitura que falha mantém o que já estava na tela: trocar por vazio apagava a lista e parava a medição.
export function reposAfterError(blocos: { servidor: { id: string }; repos: WorktreeRepo[] }[], serverId: string): WorktreeRepo[] {
  return blocos.find((b) => b.servidor.id === serverId)?.repos ?? [];
}

export const worktreesSizeBytes = (ws: WorktreeStatus[]) => worktreesSizeTotal(ws).bytes;

export function worktreesSizeSum(ws: WorktreeStatus[]): string {
  const { bytes, partial } = worktreesSizeTotal(ws);
  return partial ? m.worktree_tamanho_parcial({ tamanho: fmtBytes(bytes) }) : fmtBytes(bytes);
}

// Rótulo e cor de cada estado da worktree, iguais na lista, no disco e na janela de detalhe.
export const worktreeStateLabel: Record<WorktreeState, () => string> = {
  gone: m.worktree_estado_sumida,
  session: m.worktree_estado_em_uso,
  residual: m.worktree_estado_residual,
  dirty: m.worktree_estado_nao_commitado,
  merged: m.worktree_estado_mesclada,
  detached: m.worktree_estado_sem_branch,
  active: m.worktree_estado_andamento,
};

export const worktreeStateColor: Record<WorktreeState, string> = {
  gone: 'var(--error)',
  session: 'var(--accent)',
  residual: 'var(--warning)',
  dirty: 'var(--warning)',
  merged: 'var(--success)',
  detached: 'var(--border-strong)',
  active: 'var(--text-muted)',
};

export function sessionStateLabel(s: State | undefined): string {
  if (s === 'working') return m.worktree_sessao_trabalhando();
  if (s === 'awaiting_input') return m.worktree_sessao_esperando();
  return s ? m.worktree_sessao_parada() : '';
}

// O hash leva o servidor: o App seleciona o dono da sessão antes de montar o Chat.
export function goToSession(serverId: string, name: string) {
  window.location.hash = `#/chat/${encodeURIComponent(serverId)}/${encodeURIComponent(name)}`;
}
