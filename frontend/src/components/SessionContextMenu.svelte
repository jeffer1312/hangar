<script lang="ts">
  import { onMount } from 'svelte';
  import { getPushSettings, setSessionMute, getBranches, openEditor, gitAction, setThenLink, clearThenLink } from '../lib/api';
  import { withServer } from '../lib/auth';
  import { copyText } from '../lib/clipboard';

  // Menu de contexto (botao direito) na linha da sessao — so desktop. O Sidebar guarda posicao/alvo
  // (`menu`) e desmonta este componente ao fechar (onClose zera `menu`); todo o estado interno morre
  // junto. Os itens Renomear/Excluir/Git chamam os callbacks do Sidebar SEM onClose — os handlers de
  // la ja fecham (menuRename/menuDelete/menuGit). Os demais itens fecham aqui mesmo.
  interface Props {
    x: number; y: number;
    name: string; serverId: string; cwd: string; thenTarget: string | null;
    chainCandidates: { name: string }[];   // sessoes do MESMO servidor, sem a propria
    onClose: () => void;
    onRename: () => void;                   // Sidebar seta editing
    onDelete: () => void;                   // Sidebar abre confirmDel
    onGit: () => void;                      // Sidebar abre GitSheet (mira/restaura servidor)
    onPickBranch: (branch: string, dirty: boolean) => void; // Sidebar: dirty->confirm, senao checkout
    onFlash: (msg: string) => void;         // toast do Sidebar
  }
  let { x, y, name, serverId, cwd, thenTarget, chainCandidates, onClose, onRename, onDelete, onGit, onPickBranch, onFlash }: Props = $props();

  const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

  // Silenciar (feature #5): estado carregado sob demanda na montagem (nao trava a abertura). null =
  // ainda carregando -> o item mostra "Silenciar" ate a resposta chegar (mesmo contrato de antes).
  // No onMount pra ler as props num closure (o menu remonta a cada abertura, entao roda 1x por menu).
  let menuMuted = $state<boolean | null>(null);
  onMount(() => {
    withServer(serverId, () => getPushSettings())
      .then((p) => { menuMuted = p.muted.includes(name); })
      .catch(() => { menuMuted = false; });
  });

  async function toggleMute() {
    const next = !menuMuted;
    onClose();
    try {
      await withServer(serverId, () => setSessionMute(name, next));
      onFlash(next ? 'notificações silenciadas' : 'notificações religadas');
    } catch (e) { onFlash(`silenciar: ${errMsg(e)}`); }
  }

  // Submenu "Trocar branch" (2a pagina do menu, evita flyout). branchView != null = mostrando a lista.
  let branchView = $state<{ list: string[]; current: string | null; dirty: boolean } | null>(null);
  let branchLoading = $state(false);
  async function loadBranches() {
    branchView = { list: [], current: null, dirty: false };
    branchLoading = true;
    try {
      const info = await withServer(serverId, () => getBranches(name));
      branchView = { list: info.branches, current: info.current, dirty: info.dirty ?? false };
    } catch (e) {
      branchView = null;
      onFlash(`branches: ${errMsg(e)}`);
    } finally {
      branchLoading = false;
    }
  }
  function pickBranch(branch: string) {
    // Mesma branch: nada a fazer. Senao delega ao Sidebar (que decide dirty->confirm ou checkout).
    if (!branchView || branch === branchView.current) { onClose(); return; }
    const dirty = branchView.dirty;
    onClose();
    onPickBranch(branch, dirty);
  }

  // Submenu "Quando terminar, enviar p/…" (feature #12): picker de sessao ALVO (mesmo servidor) +
  // texto do prompt. target=null ate escolher; pre-preenche com o alvo ja armado (o texto fica no
  // backend e nao volta por GET, entao reabrir pra editar exige redigitar).
  let chainView = $state<{ target: string | null; text: string } | null>(null);
  let chainBusy = $state(false);
  function openChain() { chainView = { target: thenTarget, text: '' }; }
  async function saveChain() {
    if (!chainView?.target) return;
    const text = chainView.text.trim();
    if (!text) return;
    const target = chainView.target;
    chainBusy = true;
    try {
      await withServer(serverId, () => setThenLink(name, target, text));
      onFlash(`encadeado → ${target}`);
    } catch (e) {
      onFlash(`encadear: ${errMsg(e)}`);
    } finally {
      chainBusy = false;
      onClose();
    }
  }
  async function removeChain() {
    onClose();
    try {
      await withServer(serverId, () => clearThenLink(name));
      onFlash('vínculo removido');
    } catch (e) {
      onFlash(`remover vínculo: ${errMsg(e)}`);
    }
  }

  function copyCwd() { copyText(cwd); onClose(); }
  async function doOpenEditor() {
    onClose();
    try { await withServer(serverId, () => openEditor(name)); }
    catch (e) { onFlash(`abrir editor: ${errMsg(e)}`); }   // 404 = backend desatualizado; 500 = CP_EDITOR/DISPLAY
  }
  async function doGitPull() {
    onClose();
    onFlash('git pull…');
    try {
      const r = await withServer(serverId, () => gitAction(name, 'pull'));
      onFlash(r.output.trim().split('\n')[0] || 'pull ok');
    } catch (e) { onFlash(`git pull: ${errMsg(e)}`); }
  }
</script>

<!-- Backdrop full-screen captura o clique-fora (e o botao direito, que fecha sem reabrir). -->
<div class="menu-backdrop" onclick={onClose} oncontextmenu={(e) => { e.preventDefault(); onClose(); }} role="presentation"></div>
<div class="ctx-menu" style="left: {x}px; top: {y}px;" role="menu">
  {#if branchView}
    <button type="button" class="ctx-back" onclick={() => (branchView = null)}>‹ Trocar branch</button>
    <div class="ctx-sep"></div>
    {#if branchLoading}
      <div class="ctx-info">carregando…</div>
    {:else if branchView.list.length}
      <div class="ctx-scroll">
        {#each branchView.list as b (b)}
          <button type="button" role="menuitem" class="ctx-branch" class:current={b === branchView.current} onclick={() => pickBranch(b)}>
            {b}{#if b === branchView.current}<span class="ctx-cur">✓</span>{/if}
          </button>
        {/each}
      </div>
    {:else}
      <div class="ctx-info">sem branches</div>
    {/if}
  {:else if chainView}
    <button type="button" class="ctx-back" onclick={() => (chainView = null)}>‹ Quando terminar, enviar p/</button>
    <div class="ctx-sep"></div>
    {#if chainCandidates.length}
      <div class="ctx-scroll">
        {#each chainCandidates as c (c.name)}
          <button
            type="button" role="menuitem" class="ctx-branch"
            class:current={c.name === chainView.target}
            onclick={() => { if (chainView) chainView.target = c.name; }}
          >{c.name}{#if c.name === chainView.target}<span class="ctx-cur">✓</span>{/if}</button>
        {/each}
      </div>
    {:else}
      <div class="ctx-info">nenhuma outra sessão neste servidor</div>
    {/if}
    <div class="ctx-sep"></div>
    <div class="ctx-chain-form">
      <input
        type="text"
        class="ctx-chain-input"
        placeholder="Prompt a enviar…"
        bind:value={chainView.text}
        onkeydown={(e) => { if (e.key === 'Enter') saveChain(); }}
        aria-label="Prompt a enviar pra sessão alvo"
      />
      <button
        type="button" class="ctx-chain-save" onclick={saveChain}
        disabled={!chainView.target || !chainView.text.trim() || chainBusy}
      >Salvar</button>
    </div>
    {#if thenTarget}
      <div class="ctx-sep"></div>
      <button type="button" role="menuitem" class="danger" onclick={removeChain}>Remover vínculo</button>
    {/if}
  {:else}
    <button type="button" role="menuitem" onclick={onRename}>Renomear</button>
    <button type="button" role="menuitem" onclick={toggleMute}>
      {menuMuted ? 'Reativar notificações' : 'Silenciar notificações'}
    </button>
    {#if cwd}
      <button type="button" role="menuitem" onclick={copyCwd}>Copiar cwd</button>
      <button type="button" role="menuitem" onclick={doOpenEditor}>Abrir no editor</button>
      <div class="ctx-sep"></div>
      <button type="button" role="menuitem" onclick={onGit}>Git<span class="ctx-more">›</span></button>
      <button type="button" role="menuitem" onclick={doGitPull}>Git pull</button>
      <button type="button" role="menuitem" onclick={loadBranches}>Trocar branch<span class="ctx-more">›</span></button>
    {/if}
    <div class="ctx-sep"></div>
    <button type="button" role="menuitem" onclick={openChain}>
      {thenTarget ? `Encadeado → ${thenTarget}` : 'Quando terminar, enviar p/…'}<span class="ctx-more">›</span>
    </button>
    <div class="ctx-sep"></div>
    <button type="button" role="menuitem" class="danger" onclick={onDelete}>Excluir</button>
  {/if}
</div>

<style>
  /* ── Menu de contexto ── */
  /* .menu-backdrop e .ctx-sep tambem existem no Sidebar (kebab os reusa) — duplicados de proposito,
     escopos separados. */
  .menu-backdrop { position: fixed; inset: 0; z-index: 40; }
  .ctx-menu {
    position: fixed; z-index: 41; min-width: 168px; padding: 4px;
    display: flex; flex-direction: column;
    background: var(--bg-elevated); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); box-shadow: 0 8px 28px rgba(0,0,0,0.4);
  }
  .ctx-menu button {
    height: 32px; padding: 0 10px; text-align: left; justify-content: flex-start;
    color: var(--text-primary); font-size: var(--text-sm); border-radius: var(--radius-sm);
  }
  .ctx-menu button:hover { background: var(--bg-hover); }
  .ctx-menu button.danger { color: var(--error); }
  .ctx-menu button.danger:hover { background: rgba(255,69,58,0.12); }
  .ctx-sep { height: 1px; margin: 4px 6px; background: var(--border-subtle); }
  /* Item que abre submenu: chevron a direita. */
  .ctx-more { margin-left: auto; color: var(--text-muted); padding-left: var(--space-3); }
  .ctx-back { color: var(--text-secondary); font-weight: 600; }
  .ctx-info { padding: 6px 10px; font-size: var(--text-sm); color: var(--text-muted); }
  /* Lista de branches rolavel (repo com muitas branches nao estoura a tela). */
  .ctx-scroll { max-height: 260px; overflow-y: auto; display: flex; flex-direction: column; }
  .ctx-branch { font-family: var(--font-mono); font-size: var(--text-xs); }
  .ctx-branch.current { color: var(--accent); }
  .ctx-cur { margin-left: auto; padding-left: var(--space-2); }
  /* Feature #12: form do encadeamento (alvo escolhido acima na lista + texto do prompt). */
  .ctx-chain-form { display: flex; gap: 4px; padding: 4px 6px; }
  .ctx-chain-input {
    flex: 1; min-width: 0; height: 28px; padding: 0 8px; font-size: var(--text-sm);
    color: var(--text-primary); background: var(--bg-base); border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
  }
  .ctx-chain-save {
    height: 28px; padding: 0 10px; font-size: var(--text-sm); font-weight: 600;
    color: var(--accent); background: var(--accent-dim); border-radius: var(--radius-sm);
  }
  .ctx-chain-save:disabled { opacity: 0.5; }
</style>
