// Estado + ações git compartilhados entre os containers (mobile GitSheet / futuro desktop dock).
// .svelte.ts permite runes fora de componente.
import {
  getBranches, checkoutBranch, gitAction, getGitLog, getChangedFiles,
  commitFiles, gitPush, discardFile,
  gitRevert, gitCherryPick, gitReset, gitCreateBranch, gitCreateTag,
  type GitAction, type ChangedFile, type GitCommit, type GitResetMode,
} from './api';

// Mensagem de erro legivel: tira o prefixo "409: "/"400: " do status HTTP. Export nomeado —
// CommitMenu precisa do MESMO tratamento (getCommitBranches) sem duplicar a regex.
export const cleanErr = (e: unknown) =>
  (e instanceof Error ? e.message : 'falhou').replace(/^\d+:\s*/, '');

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
  // Sequenciador em andamento: revert/cherry-pick que conflitou -> toolbar mostra o abort.
  let pendingAbort = $state<'revert-abort' | 'cherry-pick-abort' | ''>('');
  // Busca no log. UM estado só: `logQuery` vazia = lista completa com grafo. Nao existe
  // `logFiltered` separado nem `filtered` no payload — seria a mesma informacao em 3 lugares,
  // com chance de discordarem.
  let logQuery = $state('');

  async function refresh() {
    const [b, f] = await Promise.all([getBranches(sessionName), getChangedFiles(sessionName)]);
    branches = b.branches; current = b.current; remotes = b.remotes ?? [];
    dirty = b.dirty ?? false; files = f.files;
  }
  async function load() {
    loading = true; error = ''; output = ''; pendingAbort = ''; logQuery = '';
    try { await refresh(); } catch (e) { error = cleanErr(e); } finally { loading = false; }
  }
  async function pick(b: string) {
    if (b === current || busy) return;
    busy = b; error = ''; output = '';
    try { current = (await checkoutBranch(sessionName, b)).current; await refresh(); }
    catch (e) { error = cleanErr(e); } finally { busy = ''; }
  }
  // Faz o mesmo que runAction, mas DEVOLVE {ok, output} — runAction engole o ok (so grava em
  // output). Em vez de mudar o contrato dele (tem outros callers), acrescenta ao lado; runAction
  // vira um wrapper que descarta o retorno, entao nenhum caller existente muda.
  async function runActionResult(action: GitAction) {
    if (busy) return null;
    busy = action; error = ''; output = '';
    try {
      const r = await gitAction(sessionName, action);
      output = r.output || (r.ok ? 'ok' : 'sem saída');
      await refresh();
      return r;
    } catch (e) { error = cleanErr(e); return null; } finally { busy = ''; }
  }
  async function runAction(action: GitAction) {
    await runActionResult(action);
  }
  async function openLog() {
    error = '';
    try { commits = (await getGitLog(sessionName, logQuery || undefined)).commits; }
    catch (e) { error = cleanErr(e); }
  }
  // Helper interno pras acoes de commit: busy/error/output + refresh/openLog.
  // Devolve 'ok' | 'conflito' | 'erro' | 'ocupado' — nao um booleano: quem chama precisa distinguir
  // "o git entrou em sequencer" (oferece abort) de "nem comecou" (tree suja, sha invalido, rede).
  async function _repoOp(kind: string, fn: () => Promise<{ ok: boolean; output: string }>) {
    if (busy) return 'ocupado';
    busy = kind; error = ''; output = '';
    try {
      const r = await fn(); output = r.output || 'ok'; await openLog(); return 'ok';
    } catch (e) {
      error = cleanErr(e);
      // 409 = o git rodou e recusou (conflito de revert/cherry-pick deixa sequencer em andamento).
      // 400/404/rede = nao mexeu no repo.
      return String(e).includes('409') ? 'conflito' : 'erro';
    } finally {
      // refresh SEMPRE: um cherry-pick conflitado muda a lista de arquivos (conflitos aparecem).
      // Sem isto a tela segue mostrando o repo de antes do erro.
      busy = ''; await refresh();
    }
  }
  async function revert(sha: string) {
    const r = await _repoOp('revert', () => gitRevert(sessionName, sha));
    if (r === 'conflito') pendingAbort = 'revert-abort';   // so aqui ha sequencer pra abortar
    return r === 'ok';
  }
  async function cherryPick(sha: string) {
    const r = await _repoOp('cherry-pick', () => gitCherryPick(sessionName, sha));
    if (r === 'conflito') pendingAbort = 'cherry-pick-abort';
    return r === 'ok';
  }
  async function resetTo(sha: string, mode: GitResetMode) {
    return (await _repoOp(`reset-${mode}`, () => gitReset(sessionName, sha, mode))) === 'ok';
  }
  async function createBranch(name: string, sha?: string) {
    return (await _repoOp(name, () => gitCreateBranch(sessionName, { name, ...(sha ? { sha } : {}) }))) === 'ok';
  }
  async function createTag(name: string, sha?: string, message?: string) {
    return (await _repoOp(name, () => gitCreateTag(sessionName, { name, ...(sha ? { sha } : {}), ...(message ? { message } : {}) }))) === 'ok';
  }
  // git_action NAO levanta em returncode != 0 — devolve {ok:false} (git_ops.py:201-206). Olhar so o
  // `error` faria um abort recusado ("no revert in progress") sumir o botao calado.
  async function abortOp() {
    if (!pendingAbort || busy) return false;
    const r = await runActionResult(pendingAbort);
    if (r?.ok) { pendingAbort = ''; await openLog(); return true; }
    if (r && !r.ok) error = r.output || 'abort recusado pelo git';
    return false;
  }
  async function searchLog(q: string) {
    logQuery = q;
    await openLog();
  }
  async function doCommit(message: string, paths: string[], opts?: { amend?: boolean; newBranch?: string }) {
    if (busy) return false;
    busy = 'commit'; error = ''; output = '';
    try { const r = await commitFiles(sessionName, message, paths, opts); output = r.output || 'commit ok'; await refresh(); await openLog(); return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
  async function doPush() {
    if (busy) return false;
    busy = 'push'; error = ''; output = '';
    try { const r = await gitPush(sessionName); output = r.output || 'push ok'; return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
  async function discard(path: string) {
    if (busy) return false;
    busy = path; error = '';
    try { await discardFile(sessionName, path); await refresh(); return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }

  return {
    get sessionName() { return sessionName; },
    get branches() { return branches; }, get remotes() { return remotes; },
    get current() { return current; }, get dirty() { return dirty; },
    get files() { return files; }, get commits() { return commits; },
    get loading() { return loading; },
    get busy() { return busy; }, set busy(v: string) { busy = v; },
    get error() { return error; }, set error(v: string) { error = v; },
    get output() { return output; },
    get pendingAbort() { return pendingAbort; },
    get logQuery() { return logQuery; },
    load, refresh, pick, runAction, openLog, doCommit, doPush, discard,
    revert, cherryPick, resetTo, createBranch, createTag, abortOp, searchLog,
  };
}

export type GitStore = ReturnType<typeof createGitStore>;
