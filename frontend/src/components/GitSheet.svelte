<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import {
    getBranches, checkoutBranch, gitAction,
    getChangedFiles, getFileDiff, discardFile,
    type GitAction, type ChangedFile,
  } from '../lib/api';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  let branches = $state<string[]>([]);
  let remotes = $state<string[]>([]);   // remotas sem local (nome curto); trocar pra uma cria a local (DWIM)
  let current = $state<string | null>(null);
  let dirty = $state(false);            // working tree suja -> avisa antes de trocar
  let files = $state<ChangedFile[]>([]); // arquivos com mudanca nao-commitada
  let loading = $state(false);
  let busy = $state('');        // branch/acao/arquivo em andamento (trava os botoes)
  let error = $state('');
  let output = $state('');      // saida do status/pull/fetch/stash

  let filter = $state('');      // busca que filtra as branches
  let confirmDiscard = $state('');  // path aguardando confirmacao de descarte
  // Diff aberto: quando setado, a sheet mostra o visualizador no lugar da lista.
  let diffPath = $state('');
  let diffText = $state('');
  let diffLoading = $state(false);

  const q = $derived(filter.trim().toLowerCase());
  // Branch atual sempre no topo das locais; depois a ordem por recencia que vem do backend.
  const localList = $derived.by(() => {
    const ordered = current ? [current, ...branches.filter((b) => b !== current)] : branches;
    return q ? ordered.filter((b) => b.toLowerCase().includes(q)) : ordered;
  });
  const remoteList = $derived(q ? remotes.filter((b) => b.toLowerCase().includes(q)) : remotes);

  function cleanErr(e: unknown): string {
    const m = e instanceof Error ? e.message : 'falhou';
    return m.replace(/^\d+:\s*/, '');   // tira o prefixo "409: " do status HTTP
  }

  // So recarrega os dados (branches/arquivos) — NAO mexe em loading/output, pra um refresh
  // pos-acao nao piscar a tela nem apagar a saida que acabou de sair.
  async function refresh() {
    const [b, f] = await Promise.all([getBranches(sessionName), getChangedFiles(sessionName)]);
    branches = b.branches;
    current = b.current;
    remotes = b.remotes ?? [];
    dirty = b.dirty ?? false;
    files = f.files;
  }

  async function load() {
    loading = true;
    error = '';
    output = '';
    try {
      await refresh();
    } catch (e) {
      error = cleanErr(e);
    } finally {
      loading = false;
    }
  }

  // Recarrega a cada abertura (o estado do repo pode ter mudado fora do app). Fecha o diff/busca.
  $effect(() => {
    if (open) { filter = ''; diffPath = ''; confirmDiscard = ''; load(); }
  });

  async function pick(b: string) {
    if (b === current || busy) return;
    busy = b;
    error = '';
    output = '';
    try {
      const r = await checkoutBranch(sessionName, b);
      current = r.current;
      await refresh();   // uma remota vira local (DWIM) e sai de `remotes`; atualiza dirty
    } catch (e) {
      error = cleanErr(e);
    } finally {
      busy = '';
    }
  }

  async function runAction(action: GitAction) {
    if (busy) return;
    busy = action;
    error = '';
    output = '';
    try {
      const r = await gitAction(sessionName, action);
      output = r.output || (r.ok ? 'ok' : 'sem saída');
      await refresh();
    } catch (e) {
      error = cleanErr(e);
    } finally {
      busy = '';
    }
  }

  async function openDiff(path: string) {
    if (busy) return;
    diffPath = path;
    diffText = '';
    diffLoading = true;
    error = '';
    try {
      diffText = (await getFileDiff(sessionName, path)).diff;
    } catch (e) {
      error = cleanErr(e);
      diffPath = '';   // falhou -> nao entra no visualizador
    } finally {
      diffLoading = false;
    }
  }

  async function doDiscard(path: string) {
    busy = path;
    error = '';
    try {
      await discardFile(sessionName, path);
      confirmDiscard = '';
      if (diffPath === path) diffPath = '';
      await refresh();
    } catch (e) {
      error = cleanErr(e);
    } finally {
      busy = '';
    }
  }

  // Rotulo curto do status XY do porcelain (M/A/D/R/? -> palavra).
  function fileTag(code: string): string {
    const c = code.trim()[0] ?? '';
    return { M: 'mod', A: 'novo', D: 'del', R: 'ren', C: 'copia', U: 'conflito', '?': 'novo' }[c] ?? c;
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Git">
  {#if diffPath}
    <!-- Visualizador de diff: ocupa a sheet no lugar da lista (volta pelo botao). -->
    <div class="git">
      <div class="git-head">
        <button class="git-back" onclick={() => (diffPath = '')} aria-label="Voltar">‹ voltar</button>
        <span class="git-diff-name">{diffPath}</span>
      </div>
      {#if diffLoading}
        <p class="git-muted">carregando diff…</p>
      {:else}
        <pre class="git-diff">{#each diffText.split('\n') as line, i (i)}<span
            class:add={line.startsWith('+') && !line.startsWith('+++')}
            class:del={line.startsWith('-') && !line.startsWith('---')}
            class:hunk={line.startsWith('@@')}
            class:meta={line.startsWith('diff ') || line.startsWith('index ') || line.startsWith('+++') || line.startsWith('---')}
          >{line || ' '}</span>{/each}</pre>
      {/if}
    </div>
  {:else}
    <div class="git">
      <!-- HEADER FIXO: titulo + acoes + busca nao rolam junto com a lista -->
      <div class="git-head">
        <h2 class="git-title">Git</h2>
        <div class="git-actions">
          <button class="git-act" disabled={!!busy} onclick={() => runAction('status')}>status</button>
          <button class="git-act" disabled={!!busy} onclick={() => runAction('fetch')}>fetch</button>
          <button class="git-act" disabled={!!busy} onclick={() => runAction('pull')}>pull</button>
          <button class="git-act" disabled={!!busy} onclick={() => runAction('stash')} title="guarda as mudanças (git stash)">stash</button>
          <button class="git-act" disabled={!!busy} onclick={() => runAction('stash-pop')} title="reaplica o último stash">pop</button>
        </div>
        {#if branches.length > 6 || remotes.length}
          <input
            class="git-search"
            type="search"
            placeholder="filtrar branch…"
            bind:value={filter}
            autocapitalize="off"
            autocorrect="off"
            spellcheck="false"
          />
        {/if}
      </div>

      {#if loading}
        <p class="git-muted">carregando…</p>
      {:else}
        <!-- CORPO SCROLLÁVEL -->
        <div class="git-scroll">
          {#if dirty && files.length}
            <div class="git-warn">working tree suja — troque de branch só depois de commit ou stash</div>
            <p class="git-section">{files.length} arquivo{files.length > 1 ? 's' : ''} alterado{files.length > 1 ? 's' : ''}</p>
            <div class="git-files">
              {#each files as f (f.path)}
                <div class="git-file-row" class:danger={confirmDiscard === f.path}>
                  <button class="git-file" disabled={!!busy} onclick={() => openDiff(f.path)} title="ver diff">
                    <span class="git-file-tag" data-t={fileTag(f.code)}>{fileTag(f.code)}</span>
                    <span class="git-name">{f.path}</span>
                  </button>
                  {#if confirmDiscard === f.path}
                    <button class="git-mini danger" disabled={!!busy} onclick={() => doDiscard(f.path)}>descartar</button>
                    <button class="git-mini" disabled={!!busy} onclick={() => (confirmDiscard = '')}>não</button>
                  {:else}
                    <button class="git-mini" disabled={!!busy} onclick={() => (confirmDiscard = f.path)} aria-label="descartar mudanças" title="descartar mudanças">⟲</button>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}

          <div class="git-branches">
            {#each localList as b (b)}
              <button class="git-branch" class:current={b === current} disabled={!!busy} onclick={() => pick(b)}>
                <span class="git-dot" aria-hidden="true">{b === current ? '●' : '○'}</span>
                <span class="git-name">{b}</span>
                {#if busy === b}<span class="git-spin" aria-hidden="true">…</span>{/if}
              </button>
            {/each}
            {#if !localList.length}<p class="git-muted">{q ? 'nenhuma branch local com esse filtro' : 'nenhuma branch local'}</p>{/if}
          </div>

          {#if remoteList.length}
            <p class="git-section">remotas</p>
            <div class="git-branches">
              {#each remoteList as b (b)}
                <button class="git-branch git-remote" disabled={!!busy} onclick={() => pick(b)} title="cria uma branch local rastreando a remota">
                  <span class="git-dot" aria-hidden="true">○</span>
                  <span class="git-name">{b}</span>
                  <span class="git-badge">remote</span>
                  {#if busy === b}<span class="git-spin" aria-hidden="true">…</span>{/if}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      {#if output}<pre class="git-output">{output}</pre>{/if}
      {#if error}<p class="git-error">{error}</p>{/if}
    </div>
  {/if}
</BottomSheet>

<style>
  .git { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; min-height: 0; }

  /* Header fixo: nao rola com a lista. */
  .git-head { display: flex; flex-direction: column; gap: var(--space-2); flex-shrink: 0; }
  .git-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); margin: 0; }

  .git-actions { display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .git-act {
    flex: 1 1 auto; min-width: 4rem; padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-secondary); font-size: var(--text-sm); font-family: var(--font-mono);
    cursor: pointer;
  }
  .git-act:disabled { opacity: 0.5; cursor: default; }

  .git-search {
    width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base);
    color: var(--text-primary); font-size: var(--text-sm); font-family: var(--font-mono);
  }
  .git-search::placeholder { color: var(--text-muted); }

  /* So a lista rola. max-height limita a altura (o sheet no mobile nao tem overflow proprio). */
  .git-scroll {
    overflow-y: auto; max-height: 52vh; min-height: 0;
    overscroll-behavior: contain; -webkit-overflow-scrolling: touch;
    display: flex; flex-direction: column; gap: var(--space-1);
  }
  @media (min-width: 820px) { .git-scroll { max-height: 68vh; } }

  .git-warn {
    padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--warning, #d9a441) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--warning, #d9a441) 40%, transparent);
    color: var(--text-secondary); font-size: var(--text-xs); line-height: 1.4;
  }

  .git-section {
    margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.05em;
  }

  /* ── arquivos alterados ── */
  .git-files { display: flex; flex-direction: column; gap: 2px; }
  .git-file-row { display: flex; align-items: center; gap: var(--space-2); }
  .git-file-row.danger { background: color-mix(in srgb, var(--error) 12%, transparent); border-radius: var(--radius-md); }
  .git-file {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2);
    padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .git-file:disabled { cursor: default; }
  .git-file-tag {
    flex-shrink: 0; font-size: 10px; font-family: var(--font-mono); text-transform: uppercase;
    letter-spacing: 0.03em; color: var(--text-muted); min-width: 2.4rem;
  }
  .git-file-tag[data-t="novo"] { color: var(--accent); }
  .git-file-tag[data-t="del"] { color: var(--error); }
  .git-mini {
    flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer;
  }
  .git-mini:disabled { opacity: 0.5; cursor: default; }
  .git-mini.danger { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }

  /* ── branches ── */
  .git-branches { display: flex; flex-direction: column; gap: 2px; }
  .git-branch {
    display: flex; align-items: center; gap: var(--space-2); width: 100%;
    padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .git-branch:disabled { cursor: default; }
  .git-branch.current { color: var(--text-primary); background: var(--bg-elevated); }
  .git-remote .git-dot { color: var(--text-muted); }   /* remota nao e a atual -> dot apagado */
  .git-dot { font-size: 10px; color: var(--accent); flex-shrink: 0; }
  .git-name { font-family: var(--font-mono); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
  .git-badge {
    margin-left: auto; font-size: 10px; font-family: var(--font-mono);
    color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em;
  }
  .git-spin { margin-left: auto; color: var(--text-muted); }

  /* ── diff viewer ── */
  .git-back {
    align-self: flex-start; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer;
  }
  .git-diff-name {
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .git-diff {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--bg-base); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.5;
    max-height: 62vh; overflow: auto; white-space: pre;
  }
  .git-diff span { display: block; }
  .git-diff .add { color: #4ec9b0; background: color-mix(in srgb, #4ec9b0 10%, transparent); }
  .git-diff .del { color: #f07178; background: color-mix(in srgb, #f07178 10%, transparent); }
  .git-diff .hunk { color: var(--accent); }
  .git-diff .meta { color: var(--text-muted); }

  .git-output {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--bg-elevated); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: pre-wrap; word-break: break-all; max-height: 200px; overflow: auto; flex-shrink: 0;
  }
  .git-error {
    margin: 0; font-size: var(--text-sm); color: var(--error);
    white-space: pre-wrap; word-break: break-word; flex-shrink: 0;
  }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
</style>
