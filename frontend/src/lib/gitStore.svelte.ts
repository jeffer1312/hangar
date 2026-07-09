// Estado + ações git compartilhados entre os containers (mobile GitSheet / futuro desktop dock).
// .svelte.ts permite runes fora de componente.
import {
  getBranches, checkoutBranch, gitAction, getGitLog, getChangedFiles,
  commitFiles, gitPush,
  type GitAction, type ChangedFile, type GitCommit,
} from './api';

export function createGitStore(sessionName: string) {
  let branches = $state<string[]>([]);
  let remotes = $state<string[]>([]);
  let current = $state<string | null>(null);
  let dirty = $state(false);
  let files = $state<ChangedFile[]>([]);
  let commits = $state<GitCommit[]>([]);
  let loading = $state(false);
  let busy = $state('');
  let error = $state('');
  let output = $state('');

  const cleanErr = (e: unknown) =>
    (e instanceof Error ? e.message : 'falhou').replace(/^\d+:\s*/, '');

  async function refresh() {
    const [b, f] = await Promise.all([getBranches(sessionName), getChangedFiles(sessionName)]);
    branches = b.branches; current = b.current; remotes = b.remotes ?? [];
    dirty = b.dirty ?? false; files = f.files;
  }
  async function load() {
    loading = true; error = ''; output = '';
    try { await refresh(); } catch (e) { error = cleanErr(e); } finally { loading = false; }
  }
  async function pick(b: string) {
    if (b === current || busy) return;
    busy = b; error = ''; output = '';
    try { current = (await checkoutBranch(sessionName, b)).current; await refresh(); }
    catch (e) { error = cleanErr(e); } finally { busy = ''; }
  }
  async function runAction(action: GitAction) {
    if (busy) return;
    busy = action; error = ''; output = '';
    try { const r = await gitAction(sessionName, action); output = r.output || (r.ok ? 'ok' : 'sem saída'); await refresh(); }
    catch (e) { error = cleanErr(e); } finally { busy = ''; }
  }
  async function openLog() {
    error = '';
    try { commits = (await getGitLog(sessionName)).commits; }
    catch (e) { error = cleanErr(e); }
  }
  async function doCommit(message: string, paths: string[]) {
    if (busy) return false;
    busy = 'commit'; error = ''; output = '';
    try { const r = await commitFiles(sessionName, message, paths); output = r.output || 'commit ok'; await refresh(); await openLog(); return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
  async function doPush() {
    if (busy) return false;
    busy = 'push'; error = ''; output = '';
    try { const r = await gitPush(sessionName); output = r.output || 'push ok'; return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }

  return {
    get branches() { return branches; }, get remotes() { return remotes; },
    get current() { return current; }, get dirty() { return dirty; },
    get files() { return files; }, get commits() { return commits; },
    get loading() { return loading; },
    get busy() { return busy; }, set busy(v: string) { busy = v; },
    get error() { return error; }, set error(v: string) { error = v; },
    get output() { return output; },
    load, refresh, pick, runAction, openLog, doCommit, doPush,
  };
}
