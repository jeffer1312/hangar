<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import FolderScanner from './FolderScanner.svelte';
  import { getSessions } from '../lib/api';
  import { basename } from '../lib/format';
  import { selectServer, getActiveId, serverColor } from '../lib/auth';
  import type { Server } from '../lib/auth';
  import type { SessionInfo } from '../lib/types';

  interface Props {
    open: boolean;
    servers: Server[];
    onClose: () => void;
    onCreate: (name: string, cwd?: string) => Promise<void>;
    onOpenSession: (name: string) => void;
  }
  let { open, servers, onClose, onCreate, onOpenSession }: Props = $props();

  // Servidor-alvo da nova sessão. Como o scanner/dedupe/criação leem o servidor ATIVO, escolher
  // aqui = selectServer(id): todas as chamadas seguintes do sheet caem nesse backend.
  let targetServer = $state(getActiveId() ?? '');
  function pickTarget(id: string) {
    targetServer = id;
    selectServer(id);
  }

  // Fluxo em dois passos: 1) escolher a pasta (scanner) -> 2) abrir a sessao ativa
  // daquele cwd, ou criar uma nova com o nome derivado do basename.
  let picked = $state<string | null>(null);
  let name = $state('');
  let checking = $state(false);
  let active = $state<SessionInfo | null>(null);
  let loading = $state(false);
  let error = $state('');

  // Escape hatch: digitar o caminho na mao.
  let manualOpen = $state(false);
  let manualPath = $state('');

  // Zera tudo a cada abertura. Fixa o servidor-alvo (ativo atual ou o 1º) e o seleciona, pra o
  // scanner do passo 1 já varrer o backend certo.
  $effect(() => {
    if (open) {
      picked = null;
      name = '';
      active = null;
      error = '';
      checking = false;
      loading = false;
      manualOpen = false;
      manualPath = '';
      const cur = getActiveId();
      const target = servers.find((s) => s.id === cur) ? cur! : servers[0]?.id ?? '';
      if (target) pickTarget(target);
    }
  });

  // Dedupe vs sessoes vivas: se ja existe uma com este cwd, oferecemos Abrir.
  async function handlePick(p: string) {
    picked = p;
    name = basename(p);
    error = '';
    checking = true;
    try {
      const sessions = await getSessions();
      active = sessions.find((s) => s.cwd === p) ?? null;
    } catch {
      active = null;
    } finally {
      checking = false;
    }
  }

  function reset() {
    picked = null;
    active = null;
    error = '';
  }

  function submitManual(e: SubmitEvent) {
    e.preventDefault();
    const p = manualPath.trim();
    if (p) handlePick(p);
  }

  async function create() {
    if (!picked || !name.trim()) return;
    loading = true;
    error = '';
    try {
      await onCreate(name.trim(), picked);
      onClose();
    } catch (err) {
      error = err instanceof Error ? err.message : 'Erro ao criar sessão';
    } finally {
      loading = false;
    }
  }

  function openActive() {
    if (active) {
      onOpenSession(active.name);
      onClose();
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Nova sessão">
  <h2 class="sheet-title">Nova sessão</h2>

  {#if servers.length > 1}
    <div class="server-select">
      <span class="server-select-label">Servidor</span>
      <div class="server-chips">
        {#each servers as s (s.id)}
          <button
            type="button"
            class="server-chip"
            class:on={targetServer === s.id}
            style="--chip: {serverColor(s.id)};"
            onclick={() => pickTarget(s.id)}
          >
            <span class="chip-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
            {s.label}
          </button>
        {/each}
      </div>
    </div>
  {/if}

  {#if !picked}
    <!-- Passo 1: escolher a pasta -->
    <FolderScanner onPick={handlePick} />

    <div class="advanced">
      <button class="advanced-toggle" onclick={() => (manualOpen = !manualOpen)}>
        <span>Avançado: digitar caminho</span>
        <span class="chevron" class:chevron--open={manualOpen} aria-hidden="true">›</span>
      </button>
      {#if manualOpen}
        <form class="manual-form" onsubmit={submitManual}>
          <input
            type="text"
            class="field-input"
            bind:value={manualPath}
            placeholder="/home/voce/projetos/foo"
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck={false}
            aria-label="Caminho do diretório"
          />
          <button type="submit" class="manual-go" disabled={!manualPath.trim()}>Usar</button>
        </form>
      {/if}
    </div>
  {:else}
    <!-- Passo 2: pasta escolhida -->
    <div class="picked">
      <div class="picked-head">
        <span class="picked-name">{basename(picked)}</span>
        {#if active}<span class="badge-active">ativa</span>{/if}
      </div>
      <span class="picked-path">{picked}</span>
    </div>

    {#if checking}
      <p class="hint">Verificando sessões…</p>
    {:else if active}
      <p class="hint">Já existe uma sessão neste diretório.</p>
      <button class="primary-btn" onclick={openActive}>Abrir sessão</button>
      <button class="ghost-btn" onclick={reset}>Escolher outra pasta</button>
    {:else}
      <div class="field">
        <label class="field-label" for="session-name">Nome</label>
        <input
          id="session-name"
          type="text"
          class="field-input"
          bind:value={name}
          placeholder="meu-projeto"
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck={false}
          required
        />
      </div>

      {#if error}
        <p class="error-msg" role="alert">{error}</p>
      {/if}

      <button class="primary-btn" onclick={create} disabled={loading || !name.trim()}>
        {loading ? 'Criando…' : 'Nova sessão'}
      </button>
      <button class="ghost-btn" onclick={reset}>Escolher outra pasta</button>
    {/if}
  {/if}
</BottomSheet>

<style>
  .sheet-title {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-4);
  }

  /* Seletor de servidor-alvo (só multi-servidor) */
  .server-select {
    margin-bottom: var(--space-4);
  }
  .server-select-label {
    display: block;
    font-size: var(--text-sm);
    color: var(--text-secondary);
    font-weight: 500;
    margin-bottom: var(--space-2);
  }
  .server-chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .server-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    height: 34px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-full);
    border: 1px solid var(--border-default);
    background: var(--bg-surface);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    font-weight: 500;
    transition: border-color 160ms ease-out, color 160ms ease-out;
  }
  .server-chip.on {
    border-color: var(--chip);
    color: var(--text-primary);
  }
  .chip-dot {
    width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0;
  }

  /* ── Escape hatch: digitar caminho ─────────────────────────────────────── */
  .advanced {
    margin-top: var(--space-4);
    border-top: 1px solid var(--border-subtle);
    padding-top: var(--space-3);
  }

  .advanced-toggle {
    width: 100%;
    height: 44px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 var(--space-1);
    color: var(--text-secondary);
    font-size: var(--text-sm);
  }

  .chevron {
    color: var(--text-muted);
    transition: transform 180ms var(--ease-out);
  }
  .chevron--open {
    transform: rotate(90deg);
  }

  .manual-form {
    display: flex;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }
  .manual-form .field-input {
    flex: 1;
  }
  .manual-go {
    flex-shrink: 0;
    height: 44px;
    padding: 0 var(--space-4);
    border-radius: var(--radius-md);
    background: var(--accent-dim);
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .manual-go:disabled {
    opacity: 0.5;
  }

  /* ── Pasta escolhida ───────────────────────────────────────────────────── */
  .picked {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-3);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    margin-bottom: var(--space-4);
  }

  .picked-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .picked-name {
    font-size: var(--text-lg);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge-active {
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 2px 7px;
    border-radius: var(--radius-full);
    color: var(--success);
    background: var(--pill-idle-bg);
  }

  .picked-path {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hint {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    margin-bottom: var(--space-3);
  }

  /* ── Campos / botoes ───────────────────────────────────────────────────── */
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-bottom: var(--space-4);
  }

  .field-label {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    font-weight: 500;
  }

  .field-input {
    height: 44px;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px;
    padding: 0 var(--space-3);
    outline: none;
    transition: border-color 180ms var(--ease-out);
  }
  .field-input::placeholder {
    color: var(--text-muted);
  }
  .field-input:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .error-msg {
    font-size: var(--text-sm);
    color: var(--error);
    margin-bottom: var(--space-3);
  }

  .primary-btn {
    width: 100%;
    height: 50px;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    font-size: var(--text-base);
    font-weight: 600;
    transition: background 180ms var(--ease-out);
  }
  .primary-btn:active:not(:disabled) {
    background: var(--accent-press);
  }
  .primary-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .ghost-btn {
    width: 100%;
    height: 44px;
    margin-top: var(--space-2);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    border-radius: var(--radius-md);
  }
  .ghost-btn:active {
    background: var(--bg-hover);
  }
</style>
