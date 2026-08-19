<script lang="ts">
  import { onMount } from 'svelte';
  import { DropdownMenu } from 'bits-ui';
  import * as m from '../paraglide/messages';
  import { getPushSettings, setSessionMute, getBranches, openEditor, gitAction, setThenLink, clearThenLink } from '../lib/api';
  import { withServer } from '../lib/auth';
  import { copyText } from '../lib/clipboard';

  interface Props {
    x: number; y: number;
    name: string; serverId: string; cwd: string; thenTarget: string | null;
    chainCandidates: { name: string }[];
    onClose: () => void;
    onRename: () => void;
    onDelete: () => void;
    onGit: () => void;
    onLoop: () => void;
    onPickBranch: (branch: string, dirty: boolean) => void;
    onFlash: (msg: string) => void;
  }
  let { x, y, name, serverId, cwd, thenTarget, chainCandidates, onClose, onRename, onDelete, onGit, onLoop, onPickBranch, onFlash }: Props = $props();

  const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

  // open controlado: montado = aberto; fechar (Esc/click-fora/seleção) => onClose()
  let open = $state(true);
  $effect(() => { if (!open) onClose(); });

  // Anchor virtual no ponto do clique direito / kebab
  const virtualAnchor = $derived({ getBoundingClientRect: () => new DOMRect(x, y, 0, 0) });

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
      onFlash(next ? m.ctx_notif_silenciadas() : m.ctx_notif_religadas());
    } catch (e) { onFlash(m.ctx_flash_silenciar({ n: errMsg(e) })); }
  }

  // Branch submenu
  let branchOpen = $state(false);
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
      onFlash(m.ctx_flash_branches({ n: errMsg(e) }));
    } finally {
      branchLoading = false;
    }
  }
  $effect(() => {
    if (branchOpen && !branchView && !branchLoading) void loadBranches();
  });
  function pickBranch(branch: string) {
    if (!branchView || branch === branchView.current) { onClose(); return; }
    const dirty = branchView.dirty;
    onClose();
    onPickBranch(branch, dirty);
  }

  // Chain submenu — inicializado com o alvo armado (thenTarget) pra o bind não ser opcional
  let chainOpen = $state(false);
  let chainView = $state<{ target: string | null; text: string }>({ target: thenTarget, text: '' });
  let chainBusy = $state(false);
  async function saveChain() {
    if (!chainView.target) return;
    const text = chainView.text.trim();
    if (!text) return;
    const target = chainView.target;
    chainBusy = true;
    try {
      await withServer(serverId, () => setThenLink(name, target, text));
      onFlash(m.ctx_flash_encadeado({ n: target }));
    } catch (e) {
      onFlash(m.ctx_flash_encadear({ n: errMsg(e) }));
    } finally {
      chainBusy = false;
      onClose();
    }
  }
  async function removeChain() {
    onClose();
    try {
      await withServer(serverId, () => clearThenLink(name));
      onFlash(m.ctx_vinculo_removido());
    } catch (e) {
      onFlash(m.ctx_flash_remover_vinculo({ n: errMsg(e) }));
    }
  }

  function copyCwd() { copyText(cwd); onClose(); }
  async function doOpenEditor() {
    onClose();
    try { await withServer(serverId, () => openEditor(name)); }
    catch (e) { onFlash(m.ctx_flash_abrir_editor({ n: errMsg(e) })); }
  }
  async function doGitPull() {
    onClose();
    onFlash(m.ctx_flash_git_pull());
    try {
      const r = await withServer(serverId, () => gitAction(name, 'pull'));
      onFlash(r.output.trim().split('\n')[0] || m.ctx_flash_pull_ok());
    } catch (e) { onFlash(m.ctx_flash_git_pull_erro({ n: errMsg(e) })); }
  }
</script>

<DropdownMenu.Root bind:open>
  <DropdownMenu.Content
    customAnchor={virtualAnchor}
    class="ctx-menu"
    side="bottom"
    align="start"
    sideOffset={4}
    collisionPadding={8}
    onCloseAutoFocus={(e) => e.preventDefault()}
  >
    <!-- Renomear -->
    <DropdownMenu.Item onSelect={onRename}>
      {#snippet child({ props })}
        <button {...props}>{m.ctx_renomear()}</button>
      {/snippet}
    </DropdownMenu.Item>

    <DropdownMenu.Item onSelect={toggleMute}>
      {#snippet child({ props })}
        <button {...props}>{menuMuted ? m.ctx_reativar_notif() : m.ctx_silenciar_notif()}</button>
      {/snippet}
    </DropdownMenu.Item>

    {#if cwd}
      <DropdownMenu.Item onSelect={copyCwd}>
        {#snippet child({ props })}
          <button {...props}>{m.ctx_copiar_cwd()}</button>
        {/snippet}
      </DropdownMenu.Item>
      <DropdownMenu.Item onSelect={doOpenEditor}>
        {#snippet child({ props })}
          <button {...props}>{m.ctx_abrir_editor()}</button>
        {/snippet}
      </DropdownMenu.Item>
      <DropdownMenu.Separator class="ctx-sep" />
      <DropdownMenu.Item onSelect={onGit}>
        {#snippet child({ props })}
          <button {...props}>{m.sessao_git()}<span class="ctx-more">›</span></button>
        {/snippet}
      </DropdownMenu.Item>
      <DropdownMenu.Item onSelect={doGitPull}>
        {#snippet child({ props })}
          <button {...props}>{m.ctx_git_pull()}</button>
        {/snippet}
      </DropdownMenu.Item>
      <DropdownMenu.Item onSelect={onLoop}>
        {#snippet child({ props })}
          <button {...props}>{m.sessao_loop_runner()}<span class="ctx-more">›</span></button>
        {/snippet}
      </DropdownMenu.Item>

      <!-- Trocar branch como Sub -->
      <DropdownMenu.Sub bind:open={branchOpen}>
        <DropdownMenu.SubTrigger>
          {#snippet child({ props })}
            <button {...props}>{m.ctx_trocar_branch()}<span class="ctx-more">›</span></button>
          {/snippet}
        </DropdownMenu.SubTrigger>
        <DropdownMenu.SubContent class="ctx-menu" sideOffset={8} collisionPadding={8}>
          {#if branchLoading}
            <div class="ctx-info">{m.board_carregando()}</div>
          {:else if branchView?.list.length}
            <div class="ctx-scroll">
              {#each branchView.list as b (b)}
                <DropdownMenu.Item onSelect={() => pickBranch(b)}>
                  {#snippet child({ props })}
                    <button {...props} class="ctx-branch" class:current={b === branchView?.current}>
                      {b}{#if b === branchView?.current}<span class="ctx-cur">✓</span>{/if}
                    </button>
                  {/snippet}
                </DropdownMenu.Item>
              {/each}
            </div>
          {:else if branchView}
            <div class="ctx-info">{m.ctx_sem_branches()}</div>
          {:else}
            <div class="ctx-info">{m.board_carregando()}</div>
          {/if}
        </DropdownMenu.SubContent>
      </DropdownMenu.Sub>
    {/if}

    <DropdownMenu.Separator class="ctx-sep" />

    <!-- Quando terminar, enviar p/... como Sub -->
    <DropdownMenu.Sub bind:open={chainOpen}>
      <DropdownMenu.SubTrigger>
        {#snippet child({ props })}
          <button {...props}>
            {thenTarget ? m.sessao_chain_encadeado({ n: thenTarget }) : m.sessao_chain_enviar()}<span class="ctx-more">›</span>
          </button>
        {/snippet}
      </DropdownMenu.SubTrigger>
      <DropdownMenu.SubContent class="ctx-menu ctx-sub" sideOffset={8} collisionPadding={8}>
        {#if chainCandidates.length}
          <div class="ctx-scroll">
            {#each chainCandidates as c (c.name)}
              <DropdownMenu.Item closeOnSelect={false} onSelect={() => { if (chainView) chainView.target = c.name; }}>
                {#snippet child({ props })}
                  <button {...props} class="ctx-branch" class:current={c.name === chainView.target}>
                    {c.name}{#if c.name === chainView.target}<span class="ctx-cur">✓</span>{/if}
                  </button>
                {/snippet}
              </DropdownMenu.Item>
            {/each}
          </div>
        {:else}
          <div class="ctx-info">{m.ctx_nenhuma_outra()}</div>
        {/if}
        <DropdownMenu.Separator class="ctx-sep" />
        <div class="ctx-chain-form">
          <input
            type="text"
            class="ctx-chain-input"
            placeholder={m.ctx_prompt_enviar()}
            bind:value={chainView.text}
            onkeydown={(e) => { if (e.key === 'Enter') void saveChain(); }}
            aria-label={m.ctx_prompt_alvo()}
          />
          <button type="button" class="ctx-chain-save" onclick={saveChain} disabled={!chainView.target || !chainView.text.trim() || chainBusy}>{m.ctx_salvar()}</button>
        </div>
        {#if thenTarget}
          <DropdownMenu.Separator class="ctx-sep" />
          <DropdownMenu.Item onSelect={removeChain}>
            {#snippet child({ props })}
              <button {...props} class="danger">{m.ctx_remover_vinculo()}</button>
            {/snippet}
          </DropdownMenu.Item>
        {/if}
      </DropdownMenu.SubContent>
    </DropdownMenu.Sub>

    <DropdownMenu.Separator class="ctx-sep" />
    <DropdownMenu.Item onSelect={onDelete}>
      {#snippet child({ props })}
        <button {...props} class="danger">{m.sessao_excluir_curto()}</button>
      {/snippet}
    </DropdownMenu.Item>
  </DropdownMenu.Content>
</DropdownMenu.Root>

<style>
  /* Mesma superfície do menu antigo — tokens do app.css */
  :global(.ctx-menu) {
    min-width: 168px; padding: 4px;
    display: flex; flex-direction: column;
    background: var(--surface-raised); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); box-shadow: 0 8px 28px rgba(0,0,0,0.4);
  }
  :global(.ctx-menu) button {
    height: 32px; padding: 0 10px; text-align: left; justify-content: flex-start;
    color: var(--text-primary); font-size: var(--text-sm); border-radius: var(--radius-sm);
    display: flex; align-items: center; width: 100%; background: transparent; border: 0; cursor: pointer;
  }
  :global(.ctx-menu) button:hover { background: var(--bg-hover); }
  :global(.ctx-menu) button.danger { color: var(--error); }
  :global(.ctx-menu) button.danger:hover { background: rgba(255,69,58,0.12); }
  :global(.ctx-sep) { height: 1px; margin: 4px 6px; background: var(--border-subtle); }
  :global(.ctx-more) { margin-left: auto; color: var(--text-muted); padding-left: var(--space-3); }
  :global(.ctx-info) { padding: 6px 10px; font-size: var(--text-sm); color: var(--text-muted); }
  :global(.ctx-scroll) { max-height: 260px; overflow-y: auto; display: flex; flex-direction: column; }
  :global(.ctx-branch) { font-family: var(--font-mono); font-size: var(--text-xs); }
  :global(.ctx-branch.current) { color: var(--accent); }
  :global(.ctx-cur) { margin-left: auto; padding-left: var(--space-2); }
  :global(.ctx-chain-form) { display: flex; gap: 4px; padding: 4px 6px; }
  :global(.ctx-chain-input) {
    flex: 1; min-width: 0; height: 28px; padding: 0 8px; font-size: var(--text-sm);
    color: var(--text-primary); background: var(--surface-inset); border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
  }
  :global(.ctx-chain-save) {
    height: 28px; padding: 0 10px; font-size: var(--text-sm); font-weight: 600;
    color: var(--accent); background: var(--accent-dim); border-radius: var(--radius-sm); border: 0; cursor: pointer;
  }
  :global(.ctx-chain-save:disabled) { opacity: 0.5; }
  /* Sub com formulário precisa de largura um pouco maior */
  :global(.ctx-sub) { min-width: 220px; }
</style>
