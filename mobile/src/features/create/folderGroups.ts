import { pickFolderRoot, type FsEntry, type WorktreeRepo, type WorktreeStatus } from '@hangar/core';

export type WorktreeRow = { entry: FsEntry; status: WorktreeStatus };
// `visible`: worktrees que ainda importam (com sessão ou não mescladas); `merged` fica atrás de um toque.
export type FolderRow = { entry: FsEntry; visible: WorktreeRow[]; merged: WorktreeRow[] };

const time = (e: FsEntry) => e.mtime ?? -Infinity;

// Worktrees soltas na raiz enchiam a lista (dezenas de pastas por repositório): elas entram dentro do
// repositório principal quando ele está na mesma lista; o grupo ordena pelo uso mais recente dele.
export function groupFolders(entries: FsEntry[], repos: WorktreeRepo[]): FolderRow[] {
  const statusByPath = new Map<string, WorktreeStatus>();
  for (const r of repos) for (const w of r.worktrees) statusByPath.set(w.path, w);
  const listed = new Set(entries.map((e) => e.path));
  const rows = new Map<string, FolderRow>();
  const nested: WorktreeRow[] = [];
  for (const entry of entries) {
    const status = statusByPath.get(entry.path);
    if (status && listed.has(status.repo) && status.repo !== entry.path) nested.push({ entry, status });
    else rows.set(entry.path, { entry, visible: [], merged: [] });
  }
  for (const w of nested.sort((a, b) => time(b.entry) - time(a.entry))) {
    const row = rows.get(w.status.repo)!;
    (w.status.sessions.length || !w.status.merged ? row.visible : row.merged).push(w);
  }
  for (const row of rows.values()) row.visible.sort((a, b) => Number(!!b.status.sessions.length) - Number(!!a.status.sessions.length));
  const latest = new Map([...rows.values()].map((r) => [r, Math.max(time(r.entry), ...[...r.visible, ...r.merged].map((w) => time(w.entry)))]));
  return [...rows.values()].sort((a, b) => latest.get(b)! - latest.get(a)!);
}

// Mesma pasta com outra grafia (barra final, `\`, letra da unidade).
export const samePath = (a: string, b: string) => pickFolderRoot([{ path: a }], b) !== null && pickFolderRoot([{ path: b }], a) !== null;

// Pastas das sessões da máquina, da usada por último para trás, sem repetir; a pessoal (`~`) fica de fora.
// A pasta pessoal é o "sem projeto": o servidor às vezes devolve o caminho inteiro em vez de `~`.
const HOME_DIR = /^(?:~|\/root|\/home\/[^/]+|\/Users\/[^/]+|[A-Za-z]:[\\/]Users[\\/][^\\/]+)[\\/]?$/;
export const isHomeDir = (cwd: string): boolean => HOME_DIR.test(cwd.trim());

export function recentCwds(sessions: readonly { cwd?: string; last_activity?: number | null }[] | undefined, limit: number): string[] {
  const out: string[] = [];
  const ordered = [...(sessions ?? [])].sort((a, b) => (b.last_activity ?? 0) - (a.last_activity ?? 0));
  for (const s of ordered) {
    const cwd = s.cwd?.trim();
    if (!cwd || isHomeDir(cwd) || out.some((p) => samePath(p, cwd))) continue;
    out.push(cwd);
    if (out.length >= limit) break;
  }
  return out;
}

// Worktree conhecida conta como o repositório dela: a worktree some depois do merge, o projeto fica.
export function recentProjects(cwds: readonly string[], repos: readonly WorktreeRepo[], exclude: string | null, limit: number): string[] {
  const out: string[] = [];
  for (const cwd of cwds) {
    const project = repos.flatMap((r) => r.worktrees).find((w) => samePath(w.path, cwd))?.repo ?? cwd;
    if ((exclude && samePath(project, exclude)) || out.some((p) => samePath(p, project))) continue;
    out.push(project);
    if (out.length >= limit) break;
  }
  return out;
}
