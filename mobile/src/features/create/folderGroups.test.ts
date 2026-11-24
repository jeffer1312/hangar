import { expect, it } from 'vitest';
import type { FsEntry, WorktreeRepo, WorktreeStatus } from '@hangar/core';
import { groupFolders, recentCwds, recentProjects } from './folderGroups';

const dir = (name: string, mtime: number): FsEntry => ({ name, path: `/p/${name}`, is_git: true, has_claude_md: false, mtime });
const wt = (name: string, extra: Partial<WorktreeStatus> = {}): WorktreeStatus => ({
  path: `/p/${name}`, repo: '/p/hangar', exists: true, branch: `b/${name}`, base: 'main', main_branch: 'main',
  merged: true, ahead: 0, dirty: 0, ignored: [], sessions: [], closed: 0, ...extra,
});

it('mais recentes primeiro e worktrees dentro do repositório: com sessão e não mescladas à vista, mescladas à parte', () => {
  const entries = [dir('forge', 5), dir('hangar', 10), dir('hangar-menu', 30), dir('hangar-anim', 20), dir('hangar-layout', 8), dir('advocacia', 40)];
  const repos: WorktreeRepo[] = [{ repo: '/p/hangar', worktrees: [
    wt('hangar-menu', { sessions: ['mobile-menu'], merged: false }), wt('hangar-anim'), wt('hangar-layout', { merged: false }),
  ] }];
  const rows = groupFolders(entries, repos);
  expect(rows.map((r) => r.entry.name)).toEqual(['advocacia', 'hangar', 'forge']);
  const hangar = rows[1];
  expect(hangar.visible.map((w) => w.entry.name)).toEqual(['hangar-menu', 'hangar-layout']);
  expect(hangar.merged.map((w) => w.entry.name)).toEqual(['hangar-anim']);
});

it('worktree cujo repositório está fora da lista continua como pasta comum', () => {
  const rows = groupFolders([dir('hangar-menu', 3)], [{ repo: '/outro/hangar', worktrees: [{ ...wt('hangar-menu'), repo: '/outro/hangar' }] }]);
  expect(rows.map((r) => r.entry.name)).toEqual(['hangar-menu']);
  expect(rows[0].visible).toEqual([]);
});

it('sem leitura de worktrees, só ordena por data', () => {
  expect(groupFolders([dir('a', 1), dir('b', 2)], []).map((r) => r.entry.name)).toEqual(['b', 'a']);
});

it('recentes: da sessão mais recente para trás, sem repetir a mesma pasta nem a pessoal', () => {
  const s = (cwd: string, last_activity: number | null) => ({ cwd, last_activity });
  expect(recentCwds([s('/p/a', 1), s('/p/b', 5), s('~', 9), s('/p/b/', 3), s('/p/c', null), s('', 8)], 3)).toEqual(['/p/b', '/p/a', '/p/c']);
  expect(recentCwds(undefined, 3)).toEqual([]);
});

it('a pasta pessoal não entra nas recentes, em nenhuma das grafias do servidor', () => {
  const s = (cwd: string, last: number) => ({ cwd, last_activity: last });
  expect(recentCwds([s('/home/ana', 9), s('/Users/ana/', 8), s('C:\\Users\\ana', 7), s('/root', 6), s('/home/ana/proj', 5)], 3))
    .toEqual(['/home/ana/proj']);
});

it('recentes: worktree conhecida vira o repositório, e a pasta escolhida sai da lista', () => {
  const repos: WorktreeRepo[] = [{ repo: '/p/hangar', worktrees: [wt('hangar-menu')] }];
  expect(recentProjects(['/p/hangar-menu', '/p/hangar', '/p/forge', '/p/web', '/p/api'], repos, '/p/forge', 3)).toEqual(['/p/hangar', '/p/web', '/p/api']);
});
