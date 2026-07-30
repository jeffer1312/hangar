// Estado + ações git do modal de git (Git.svelte -> GitTabs -> as tres abas), nas duas views.
// .svelte.ts permite runes fora de componente.
import {
  getBranches, checkoutBranch, gitAction, getGitLog, getChangedFiles,
  commitFiles, gitPush, discardFile,
  gitRevert, gitCherryPick, gitReset, gitCreateBranch, gitCreateTag,
  getFileDiff, getCommitFileDiff, getCommitDiff, getCommitDiffVsWorktree,
  type GitAction, type ChangedFile, type GitCommit, type GitResetMode,
} from './api';
// `import type` some na compilacao: o Shiki continua entrando so pelo import() dinamico la embaixo.
import type { DiffRow } from './highlight';

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
    // pendingAbort vem do DISCO (f.sequencer, lido de CHERRY_PICK_HEAD/REVERT_HEAD), nao so de
    // memoria de sessao -> sobrevive a load() reabrindo a sheet com um conflito ainda em aberto.
    pendingAbort = f.sequencer === 'revert' ? 'revert-abort'
      : f.sequencer === 'cherry-pick' ? 'cherry-pick-abort' : '';
  }
  async function load() {
    loading = true; error = ''; output = ''; logQuery = '';
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
      // git_action NAO levanta em returncode != 0 (git_ops.py) -- so `output` gravava a falha, e o
      // <pre> dela usa a MESMA cor cinza de um "ok" (ex. um `pull` com conflito/auth expirada/branch
      // divergida passava despercebido, sem nunca setar `error`). abortOp() so trata o `ok`; aqui e o
      // dono unico da mensagem de erro pra qualquer acao via runActionResult.
      if (!r.ok) error = r.output || 'sem saída';
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
      const r = await fn();
      output = r.output || 'ok';
      // openLog FORA do escopo de erro da acao: fn() ja voltou com sucesso (commit de revert/
      // cherry-pick ja gravado no disco) quando chegamos aqui -- uma falha na releitura (blip de
      // rede) nao pode rebaixar isto a 'erro', ou o usuario ve "falhou", clica de novo e duplica a
      // acao (dois reverts, dois cherry-picks).
      try { await openLog(); } catch { /* acao JA aconteceu; falha de releitura nao a desfaz */ }
      return 'ok';
    } catch (e) {
      error = cleanErr(e);
      // 409 = o git rodou e recusou (conflito de revert/cherry-pick deixa sequencer em andamento).
      // 400/404/rede = nao mexeu no repo.
      return String(e).includes('409') ? 'conflito' : 'erro';
    } finally {
      // refresh SEMPRE: um cherry-pick conflitado muda a lista de arquivos (conflitos aparecem).
      // Sem isto a tela segue mostrando o repo de antes do erro. Try/catch proprio: se o refresh
      // falhar (ex. index.lock transitorio logo apos a operacao), o throw daqui NAO pode substituir
      // o `return` do try acima e vazar pra fora de um onclick sem await (load/doCommit/discard ja
      // se protegem assim). So escreve em `error` se ele ainda estiver vazio -- senao a falha do
      // refresh (ex. index.lock) sobrescreveria o erro de verdade da acao (ex. conflito de merge).
      try { await refresh(); } catch (e) { if (!error) error = cleanErr(e); }
      // busy zera SO DEPOIS do refresh -- mesmo invariante do resto do arquivo (pick/runActionResult/
      // doCommit/discard). Destravar antes deixaria a UI aceitar um segundo comando destrutivo
      // (outro cherry-pick, outro reset --hard) enquanto o primeiro ainda recarrega.
      busy = '';
    }
  }
  async function revert(sha: string) {
    const r = await _repoOp('revert', () => gitRevert(sessionName, sha));
    // pendingAbort ja veio do DISCO via refresh() (dentro do _repoOp, `f.sequencer`). Um 409 tambem
    // acontece com a tree suja e o revert NEM COMECOU (sem REVERT_HEAD) -- so aqui sobrescreve se o
    // refresh nao tiver detectado sequenciador nenhum, senao criava um botao de abort que o git recusa.
    if (r === 'conflito' && !pendingAbort) pendingAbort = 'revert-abort';
    return r === 'ok';
  }
  async function cherryPick(sha: string) {
    const r = await _repoOp('cherry-pick', () => gitCherryPick(sessionName, sha));
    if (r === 'conflito' && !pendingAbort) pendingAbort = 'cherry-pick-abort';
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
    // r && !r.ok: error ja foi setado dentro de runActionResult (dono unico da mensagem, evita
    // duplicar o mesmo texto duas vezes).
    return false;
  }
  async function searchLog(q: string) {
    // busy trava o campo (LogSearch ja desabilita o botao em disabled={!!git.busy}), mesma convencao
    // do resto do arquivo -- sem isto, buscar "abc" e "xyz" rapido demais deixava quem RESPONDESSE
    // por ultimo vencer, nao quem foi digitado por ultimo. openLog() sozinho (chamado de dentro do
    // _repoOp, que ja segura `busy` com outro kind) nunca passa por aqui -- sem risco de deadlock.
    if (busy) return;
    busy = 'log';
    logQuery = q;
    await openLog();
    busy = '';
  }
  async function doCommit(message: string, paths: string[], opts?: { amend?: boolean; newBranch?: string }) {
    if (busy) return false;
    busy = 'commit'; error = ''; output = '';
    try {
      const r = await commitFiles(sessionName, message, paths, opts);
      output = r.output || 'commit ok';
      // refresh/openLog FORA do escopo de erro do commit: o commit ja foi gravado no disco quando
      // chegamos aqui -- uma falha na releitura nao pode virar 'erro' e levar o usuario a commitar
      // de novo (commit duplicado).
      try { await refresh(); await openLog(); } catch { /* commit JA aconteceu; releitura nao o desfaz */ }
      return true;
    }
    catch (e) { error = cleanErr(e); return false; }
    finally { busy = ''; }
  }
  async function doPush() {
    if (busy) return false;
    busy = 'push'; error = ''; output = '';
    try { const r = await gitPush(sessionName); output = r.output || 'push ok'; return true; }
    catch (e) { error = cleanErr(e); return false; } finally { busy = ''; }
  }
  // ── Diff aberto ────────────────────────────────────────────────────────────
  // Vivia duplicado no GitSheet e no GitPanel (≈130 linhas em cada). Com o modal de abas ha um dono
  // so, e as abas ficam burras: chamam, olham o boolean e decidem se descem de nivel.
  let diffPath = $state('');
  let diffRows = $state<DiffRow[]>([]);
  let diffLoading = $state(false);
  let diffSha = $state('');          // '' = diff da working tree
  let diffTruncated = $state(false); // backend cortou em 200KB

  function closeDiff() {
    diffPath = ''; diffSha = ''; diffRows = []; diffTruncated = false;
  }

  // Helper unico das quatro entradas: muda so o titulo, a chave de `busy` e o fetch.
  // `chave` e explicita de proposito: no diff de UM arquivo dentro de um commit as duas coisas
  // existem (sha e path) e o valor certo e o PATH — e o que as versoes antigas gravavam, e o que uma
  // lista destacando "este arquivo esta carregando" compara (mesmo padrao do BranchList).
  async function _abrirDiff(titulo: string, sha: string, chave: string, buscar: () => Promise<{ diff: string; truncated?: boolean }>) {
    if (busy) return false;
    diffSha = sha; diffPath = titulo; diffRows = []; diffTruncated = false;
    diffLoading = true; busy = chave; error = '';
    try {
      const r = await buscar();
      diffTruncated = !!r.truncated;
      const { highlightDiff } = await import('./highlight');   // Shiki carrega on-demand
      diffRows = await highlightDiff(r.diff, titulo);
      return true;
    } catch (e) {
      error = cleanErr(e);
      closeDiff();   // sem diff pra mostrar: quem chamou nao desce de nivel
      return false;
    } finally {
      diffLoading = false; busy = '';
    }
  }

  const openFileDiff = (path: string) =>
    _abrirDiff(path, '', path, () => getFileDiff(sessionName, path));
  // Diff de um arquivo DENTRO de um commit historico.
  const openCommitFileDiff = (sha: string, path: string) =>
    _abrirDiff(path, sha, path, () => getCommitFileDiff(sessionName, sha, path));
  // Commit INTEIRO. Titulo sintetico: o highlightDiff usa o path so pra detectar linguagem (sem
  // extensao = texto plano, que e o certo pra um diff multi-arquivo).
  const openCommitFullDiff = (c: GitCommit) =>
    _abrirDiff(`commit ${c.short}`, c.hash, c.hash, () => getCommitDiff(sessionName, c.hash));
  // Commit vs o disco agora. Titulo diferente pro usuario saber qual dos dois diffs esta vendo.
  const openCommitWorktreeDiff = (c: GitCommit) =>
    _abrirDiff(`commit ${c.short} ↔ working tree`, c.hash, c.hash, () => getCommitDiffVsWorktree(sessionName, c.hash));

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
    get diffPath() { return diffPath; }, get diffRows() { return diffRows; },
    get diffLoading() { return diffLoading; }, get diffSha() { return diffSha; },
    get diffTruncated() { return diffTruncated; },
    load, refresh, pick, runAction, openLog, doCommit, doPush, discard,
    revert, cherryPick, resetTo, createBranch, createTag, abortOp, searchLog,
    openFileDiff, openCommitFileDiff, openCommitFullDiff, openCommitWorktreeDiff, closeDiff,
  };
}

export type GitStore = ReturnType<typeof createGitStore>;
