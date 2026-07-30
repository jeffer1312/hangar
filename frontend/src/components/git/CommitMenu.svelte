<script lang="ts">
  import type { GitCommit } from '../../lib/api';
  import { getCommitFiles, getCommitBranches } from '../../lib/api';
  import { portal } from '../../lib/portal';
  import { cleanErr, type GitStore } from '../../lib/gitStore.svelte';
  import { focusableElements, nextFocusIndex } from '../../lib/focusCycle';

  interface Props {
    commit: GitCommit;
    git: GitStore;
    onClose: () => void;
    onShowDiff: (c: GitCommit) => void;
    onShowWorktreeDiff: (c: GitCommit) => void;
  }
  let { commit, git, onClose, onShowDiff, onShowWorktreeDiff }: Props = $props();

  let mode = $state<'menu' | 'branch' | 'tag' | 'reset' | 'branches'>('menu');
  let name = $state('');            // nome da branch/tag nova
  let tagMsg = $state('');          // mensagem opcional da tag (anotada)
  let confirmAct = $state('');      // 'cherry-pick' | 'revert' | 'hard' aguardando confirm
  let contains = $state<{ local: string[]; remote: string[] } | null>(null);   // branches que contem
  let containsFailed = $state(false);   // getCommitBranches falhou -> "carregando…" pararia de mentir
  let menuEl = $state<HTMLElement | null>(null);

  // O menu e portalado pro <body>, fora do sheetEl -> o Tab-trap do BottomSheet (que so conhece os
  // focaveis DENTRO do sheet) nunca alcança estes itens no desktop, e um usuario de teclado abre o
  // menu e nao consegue sair dele pro primeiro item. Foca o 1o item no mount e prende o Tab aqui
  // enquanto o menu existe (mesma logica de ciclo do BottomSheet, via focusCycle compartilhado).
  $effect(() => {
    if (menuEl) focusableElements(menuEl)[0]?.focus();
  });
  function onMenuKeydown(e: KeyboardEvent) {
    if (e.key !== 'Tab' || !menuEl) return;
    e.preventDefault();
    const elements = focusableElements(menuEl);
    if (!elements.length) return;
    const activeIndex = elements.indexOf(document.activeElement as HTMLElement);
    const nextIndex = activeIndex < 0
      ? (e.shiftKey ? elements.length - 1 : 0)
      : nextFocusIndex(activeIndex, elements.length, e.shiftKey ? -1 : 1);
    elements[nextIndex].focus();
  }

  // Fecha so no sucesso: no erro o git.error aparece no pe do menu e ele fica aberto (falha aparece).
  async function run(fn: () => Promise<boolean>) {
    if (await fn()) onClose();
  }
  async function copy(text: string) {
    // navigator.clipboard exige contexto seguro: PWA em http://IP-LAN pode nao ter -> falha aparece.
    try { await navigator.clipboard.writeText(text); onClose(); }
    catch { git.error = 'clipboard indisponível neste navegador/contexto'; }
  }

  // "Copy to clipboard" do Tortoise: hash + autor + data + mensagem + arquivos. A lista de arquivos
  // vem de getCommitFiles (rota que ja existe) — assim o texto e o MESMO abrindo o menu da lista ou
  // do detalhe; sem isso, dependeria de o detalhe ja ter sido carregado.
  async function copyDetails() {
    let files: string[] | null = null;
    try { files = (await getCommitFiles(git.sessionName, commit.hash)).files.map((f) => f.path); }
    catch { files = null; }   // falha nao impede copiar o resto — mas o texto DIZ que faltou
    await copy([
      `commit ${commit.hash}`,
      `Autor:  ${commit.author}`,
      `Data:   ${new Date(commit.ts * 1000).toLocaleString()}`,
      '',
      commit.subject,
      '',
      'Arquivos:',
      ...(files === null ? ['  (lista indisponível — falha ao ler o commit)']
                         : files.map((p) => `  ${p}`)),
    ].join('\n'));
  }

  async function loadBranches() {
    mode = 'branches';
    contains = null;
    containsFailed = false;
    // cleanErr tira o "409: " da frente; String(e) mostraria o prefixo cru.
    try { contains = await getCommitBranches(git.sessionName, commit.hash); }
    catch (e) { git.error = cleanErr(e); containsFailed = true; }
  }
</script>

<!-- Escape fecha o MENU, nao a sheet inteira. Tem que ser na fase de CAPTURA: o BottomSheet escuta
     keydown no window na fase de bolha e chama stopImmediatePropagation (BottomSheet.svelte:130-138),
     entao um listener normal registrado depois nunca rodaria. Captura no window roda antes de todos. -->
<svelte:window onkeydowncapture={(e) => {
  if (e.key === 'Escape') { e.stopImmediatePropagation(); e.preventDefault(); onClose(); }
}} />
<div use:portal class="cm-back" onclick={onClose} role="presentation"></div>
<div use:portal bind:this={menuEl} onkeydown={onMenuKeydown} tabindex="-1" class="cm" role="menu" aria-label="ações do commit {commit.short}">
  {#if mode === 'menu'}
    <p class="cm-title">commit {commit.short} — {commit.subject}</p>
    <button class="cm-item" onclick={() => { onShowDiff(commit); onClose(); }}>Ver diff completo</button>
    <button class="cm-item" onclick={() => { onShowWorktreeDiff(commit); onClose(); }}>Comparar com a working tree</button>
    <button class="cm-item" onclick={() => copy(commit.hash)}>Copiar hash</button>
    <button class="cm-item" onclick={() => copy(commit.subject)}>Copiar mensagem</button>
    <button class="cm-item" onclick={copyDetails}>Copiar detalhes completos</button>
    <button class="cm-item" onclick={loadBranches}>Branches que contêm este commit ▸</button>
    <button class="cm-item" onclick={() => (mode = 'branch')}>Criar branch aqui…</button>
    <button class="cm-item" onclick={() => (mode = 'tag')}>Criar tag aqui…</button>
    {#if confirmAct === 'cherry-pick'}
      <button class="cm-item danger" disabled={!!git.busy} onclick={() => run(() => git.cherryPick(commit.hash))}>confirmar cherry-pick</button>
      <button class="cm-item" onclick={() => (confirmAct = '')}>não</button>
    {:else}
      <button class="cm-item" onclick={() => (confirmAct = 'cherry-pick')}>Cherry-pick</button>
    {/if}
    {#if confirmAct === 'revert'}
      <button class="cm-item danger" disabled={!!git.busy} onclick={() => run(() => git.revert(commit.hash))}>confirmar revert</button>
      <button class="cm-item" onclick={() => (confirmAct = '')}>não</button>
    {:else}
      <button class="cm-item" onclick={() => (confirmAct = 'revert')}>Revert este commit</button>
    {/if}
    <button class="cm-item" onclick={() => (mode = 'reset')}>Reset até aqui ▸</button>
  {:else if mode === 'branch'}
    <p class="cm-title">branch nova em {commit.short}</p>
    <input class="cm-input" bind:value={name} placeholder="nome da branch"
      autocapitalize="off" autocorrect="off" spellcheck="false" />
    <div class="cm-row">
      <button class="cm-item primary" disabled={!name.trim() || !!git.busy}
        onclick={() => run(() => git.createBranch(name.trim(), commit.hash))}>criar</button>
      <button class="cm-item" onclick={() => { mode = 'menu'; name = ''; }}>voltar</button>
    </div>
  {:else if mode === 'tag'}
    <p class="cm-title">tag nova em {commit.short}</p>
    <input class="cm-input" bind:value={name} placeholder="nome da tag"
      autocapitalize="off" autocorrect="off" spellcheck="false" />
    <input class="cm-input" bind:value={tagMsg} placeholder="mensagem (opcional — vira tag anotada)"
      autocapitalize="off" autocorrect="off" spellcheck="false" />
    <div class="cm-row">
      <button class="cm-item primary" disabled={!name.trim() || !!git.busy}
        onclick={() => run(() => git.createTag(name.trim(), commit.hash, tagMsg.trim() || undefined))}>criar</button>
      <button class="cm-item" onclick={() => { mode = 'menu'; name = ''; tagMsg = ''; }}>voltar</button>
    </div>
  {:else if mode === 'branches'}
    <p class="cm-title">branches com {commit.short}</p>
    {#if containsFailed}
      <p class="cm-muted">não deu pra ler as branches</p>
    {:else if contains === null}
      <p class="cm-muted">carregando…</p>
    {:else if !contains.local.length && !contains.remote.length}
      <p class="cm-muted">nenhuma branch contém este commit</p>
    {:else}
      <ul class="cm-list">
        {#each contains.local as b (b)}<li>{b}</li>{/each}
        {#each contains.remote as b (b)}<li class="cm-remote">{b}</li>{/each}
      </ul>
    {/if}
    <button class="cm-item" onclick={() => { mode = 'menu'; contains = null; }}>voltar</button>
  {:else}
    <p class="cm-title">reset até {commit.short}</p>
    <button class="cm-item" disabled={!!git.busy} onclick={() => run(() => git.resetTo(commit.hash, 'soft'))}>soft — mantém tudo staged</button>
    <button class="cm-item" disabled={!!git.busy} onclick={() => run(() => git.resetTo(commit.hash, 'mixed'))}>mixed — mantém na tree, fora do stage</button>
    {#if confirmAct === 'hard'}
      <p class="cm-warn">HARD apaga mudanças não commitadas. Tem certeza?</p>
      <button class="cm-item danger" disabled={!!git.busy} onclick={() => run(() => git.resetTo(commit.hash, 'hard'))}>sim, reset --hard</button>
      <button class="cm-item" onclick={() => (confirmAct = '')}>não</button>
    {:else}
      <button class="cm-item danger" onclick={() => (confirmAct = 'hard')}>hard — descarta mudanças…</button>
    {/if}
    <button class="cm-item" onclick={() => { mode = 'menu'; confirmAct = ''; }}>voltar</button>
  {/if}
  {#if git.error}<p class="git-error">{git.error}</p>{/if}
</div>
<!-- O GitSheet/GitPanel tambem imprimem git.error no rodape (GitSheet:206, GitPanel:102). Com o menu
     aberto o mesmo texto apareceria duas vezes: os dois callers passam a esconder o rodape enquanto
     `menuCommit` existe ({#if git.error && !menuCommit}), porque o menu fica por cima. -->

<style>
  /* Overlay DENTRO do sheet (BottomSheet usa z-index 100): backdrop 110 / card 120. */
  .cm-back { position: fixed; inset: 0; z-index: 110; background: color-mix(in srgb, var(--bg-base) 60%, transparent); }
  .cm {
    position: fixed; z-index: 120; left: var(--space-3); right: var(--space-3); bottom: var(--space-3);
    display: flex; flex-direction: column; gap: 2px; padding: var(--space-2);
    border-radius: var(--radius-lg); border: 1px solid var(--border-default);
    background: var(--bg-elevated); box-shadow: 0 8px 30px rgb(0 0 0 / 0.35);
    animation: view-in 200ms var(--ease-out) both;
    /* Sem teto o menu (550px medido) some abaixo da dobra num viewport baixo (ex. celular deitado,
       400x480) sem jeito de rolar ate os itens de baixo. */
    max-height: calc(100dvh - 2 * var(--space-3)); overflow-y: auto;
  }
  @keyframes view-in { from { opacity: 0; transform: translateY(8px); } to { opacity: 1; transform: translateY(0); } }
  @media (min-width: 820px) {
    /* Centrado (nao "top: 30%"): com top fixo, um menu alto em viewport baixo comecava ACIMA da
       tela (medido: -82px em 1280x720 com top:30%=216px + 550px de altura). Centrado vertical, o
       teto acima garante que ele nunca passa da tela pra cima OU pra baixo. */
    .cm { left: 50%; right: auto; bottom: auto; top: 50%; transform: translate(-50%, -50%);
      width: 340px; animation: none; }
  }
  .cm-title { margin: 0; padding: var(--space-1) var(--space-2); font-size: var(--text-xs);
    color: var(--text-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cm-item { display: block; width: 100%; padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent; color: var(--text-secondary);
    font-size: var(--text-sm); text-align: left; cursor: pointer; }
  @media (hover: hover) { .cm-item:hover { background: var(--bg-hover); } }
  .cm-item:disabled { opacity: 0.5; cursor: default; }
  .cm-item.danger { color: var(--error); }
  .cm-item.primary { color: var(--accent); }
  .cm-row { display: flex; gap: var(--space-2); }
  .cm-input { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); }
  .cm-warn { margin: 0; padding: var(--space-2); font-size: var(--text-xs); color: var(--error); }
  .cm-muted { margin: 0; padding: var(--space-2); font-size: var(--text-sm); color: var(--text-muted); }
  .cm-list { margin: 0; padding: var(--space-1) var(--space-2); max-height: 40vh; overflow-y: auto;
    list-style: none; font-family: var(--font-mono); font-size: var(--text-sm);
    color: var(--text-secondary); }
  .cm-list li { padding: 2px 0; }
  .cm-remote { color: var(--text-muted); }
  .git-error { margin: 0; padding: var(--space-2); font-size: var(--text-sm); color: var(--error);
    white-space: pre-wrap; word-break: break-word; }
</style>
