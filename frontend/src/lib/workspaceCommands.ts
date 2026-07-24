export type WorkspaceActionGroup = 'Navegação' | 'Sessão' | 'Ferramentas' | 'Colaboração';

export interface WorkspaceAction {
  id: string;
  title: string;
  detail: string;
  keywords: string[];
  group: WorkspaceActionGroup;
  shortcut?: string;
  disabled?: boolean;
  run: () => void;
}

export interface SearchableWorkspaceItem {
  key: string;
  title: string;
  detail: string;
  keywords: string[];
  group: WorkspaceActionGroup | 'Sessões';
}

const normalize = (value: string) =>
  value.normalize('NFD').replace(/\p{Diacritic}/gu, '').toLocaleLowerCase().trim();

export function filterWorkspaceItems<T extends SearchableWorkspaceItem>(items: T[], query: string): T[] {
  const needle = normalize(query);
  if (!needle) return items;
  return items.filter((item) =>
    normalize([item.title, item.detail, ...item.keywords].join(' ')).includes(needle),
  );
}
