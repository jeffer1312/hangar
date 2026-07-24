import type { AggSession } from './types';

export type WorkspaceView = 'chat' | 'board' | 'canvas';
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
  disabled?: boolean;
}

export interface WorkspaceActionItem extends SearchableWorkspaceItem {
  kind: 'action';
  action: WorkspaceAction;
}

export interface WorkspaceSessionItem extends SearchableWorkspaceItem {
  kind: 'session';
  session: AggSession;
  group: 'Sessões';
}

export type PaletteItem = WorkspaceActionItem | WorkspaceSessionItem;

const normalize = (value: string) =>
  value.normalize('NFD').replace(/\p{Diacritic}/gu, '').toLocaleLowerCase().trim();

export function filterWorkspaceItems<T extends SearchableWorkspaceItem>(items: T[], query: string): T[] {
  const needle = normalize(query);
  if (!needle) return items;
  return items.filter((item) =>
    normalize([item.title, item.detail, ...item.keywords].join(' ')).includes(needle),
  );
}

export function workspaceSessionItems(rows: AggSession[]): WorkspaceSessionItem[] {
  return rows.map((session) => ({
    key: `${session.serverId}::${session.name}`,
    kind: 'session',
    session,
    title: session.name,
    detail: `${session.serverLabel} · ${session.cwd ?? 'sem diretório'}`,
    keywords: [session.serverId, session.serverLabel, session.cwd ?? ''],
    group: 'Sessões',
  }));
}
