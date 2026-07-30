// Navegacao do modal de git: qual aba esta ativa e em que nivel cada uma parou.
//
// Nivel, nao pilha: a profundidade maxima e 2 (Historico -> commit -> diff), 1 (Mudancas -> diff) e
// 0 (Branches). Uma pilha de navegacao seria maior que o problema.
//
// A aba ativa e guardada por ID, nunca por indice: mexer na lista de abas nao pode trocar a aba
// ativa debaixo do usuario (mesma classe do plan_name no _list_sig).

export type GitTabId = 'changes' | 'history' | 'branches';

export const GIT_TABS = [
  { id: 'changes',  label: 'Mudanças',  maxLevel: 1 },
  { id: 'history',  label: 'Histórico', maxLevel: 2 },
  { id: 'branches', label: 'Branches',  maxLevel: 0 },
] as const satisfies readonly { id: GitTabId; label: string; maxLevel: number }[];

export interface GitNav {
  tab: GitTabId;
  levels: Record<GitTabId, number>;
}

const maxOf = (tab: GitTabId): number => GIT_TABS.find((t) => t.id === tab)!.maxLevel;

export function initialNav(): GitNav {
  return { tab: 'changes', levels: { changes: 0, history: 0, branches: 0 } };
}

export function selectTab(nav: GitNav, tab: GitTabId): GitNav {
  return { tab, levels: { ...nav.levels } };
}

export function currentLevel(nav: GitNav): number {
  return nav.levels[nav.tab];
}

export function pushLevel(nav: GitNav): GitNav {
  const teto = maxOf(nav.tab);
  return { tab: nav.tab, levels: { ...nav.levels, [nav.tab]: Math.min(nav.levels[nav.tab] + 1, teto) } };
}

export function popLevel(nav: GitNav): GitNav {
  return { tab: nav.tab, levels: { ...nav.levels, [nav.tab]: Math.max(nav.levels[nav.tab] - 1, 0) } };
}
