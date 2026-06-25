<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';

  interface Props {
    open: boolean;
    onClose: () => void;
    onCreate: (name: string, cwd?: string) => Promise<void>;
  }
  let { open, onClose, onCreate }: Props = $props();

  let name = $state('');
  let cwd = $state('');
  let loading = $state(false);
  let error = $state('');

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    loading = true;
    error = '';
    try {
      await onCreate(name.trim(), cwd.trim() || undefined);
      name = '';
      cwd = '';
      onClose();
    } catch (err) {
      error = err instanceof Error ? err.message : 'Erro ao criar sessão';
    } finally {
      loading = false;
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Nova sessão">
  <h2 class="sheet-title">Nova sessão</h2>

  <form onsubmit={handleSubmit} class="sheet-form">
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

    <div class="field">
      <label class="field-label" for="session-cwd">Diretório</label>
      <input
        id="session-cwd"
        type="text"
        class="field-input"
        bind:value={cwd}
        placeholder="~/projetos/foo (opcional)"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck={false}
      />
    </div>

    {#if error}
      <p class="error-msg" role="alert">{error}</p>
    {/if}

    <button
      type="submit"
      class="create-btn"
      disabled={loading || !name.trim()}
    >
      {loading ? 'Criando…' : 'Criar sessão'}
    </button>
  </form>
</BottomSheet>

<style>
  .sheet-title {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-5);
  }

  .sheet-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
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
    transition: border-color 180ms ease-out;
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
  }

  .create-btn {
    width: 100%;
    height: 50px;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    font-size: var(--text-base);
    font-weight: 600;
    transition: background 180ms ease-out;
  }

  .create-btn:active:not(:disabled) {
    background: var(--accent-press);
  }

  .create-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
