<script lang="ts">
  import { getPane, sendKey, type NavKey } from '../lib/api';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  let text = $state('');
  let busy = $state(false);
  let err = $state<string | null>(null);

  // Poll do pane CRU enquanto aberto. O overlay so-TUI nao gera evento no .jsonl/SSE, entao a unica
  // fonte viva e o capture-pane. ~450ms = responsivo sem martelar o backend (1 subprocess por poll).
  $effect(() => {
    if (!open) return;
    let alive = true;
    async function tick() {
      try {
        const t = await getPane(sessionName);
        if (alive) { text = t; err = null; }
      } catch (e) {
        if (alive) err = e instanceof Error ? e.message : 'erro';
      }
    }
    tick();
    const id = setInterval(tick, 450);
    return () => { alive = false; clearInterval(id); };
  });

  async function press(key: NavKey) {
    if (busy) return;
    busy = true;
    try {
      await sendKey(sessionName, key);
      // Refresh imediato pra feedback instantaneo (nao espera o proximo tick do poll).
      text = await getPane(sessionName);
    } catch (e) {
      err = e instanceof Error ? e.message : 'erro';
    } finally {
      busy = false;
    }
  }

  // Linhas de chrome do rodape (statusline + box de input) sao ruido aqui — mas mantemos o pane
  // INTEIRO pra nao esconder nada do overlay. Trim so as linhas vazias do fim.
  const lines = $derived(text.replace(/\s+$/, '').split('\n'));
</script>

{#if open}
  <div class="tm-backdrop" role="dialog" aria-modal="true" aria-label="Terminal (overlay TUI)">
    <header class="tm-head">
      <button class="tm-back" onclick={onClose} aria-label="Voltar ao chat">
        <span class="tm-back-arrow">←</span> Voltar ao chat
      </button>
      <span class="tm-title">⌨ {sessionName}</span>
    </header>

    <div class="tm-screen">
      {#if err}
        <p class="tm-err">{err}</p>
      {/if}
      <pre class="tm-pane">{lines.join('\n')}</pre>
    </div>

    <nav class="tm-keys" aria-label="Teclas de resgate">
      <span class="tm-keys-hint">resgate</span>
      <button class="tm-key" onclick={() => press('Escape')}>Esc</button>
      <button class="tm-key" onclick={() => press('Tab')}>⇥</button>
      <div class="tm-arrows">
        <button class="tm-key" onclick={() => press('Left')}>←</button>
        <button class="tm-key" onclick={() => press('Up')}>↑</button>
        <button class="tm-key" onclick={() => press('Down')}>↓</button>
        <button class="tm-key" onclick={() => press('Right')}>→</button>
      </div>
      <button class="tm-key tm-enter" onclick={() => press('Enter')}>⏎</button>
    </nav>
  </div>
{/if}

<style>
  /* Overlay fullscreen (NAO bottom sheet): o pane e largo (200 cols) e precisa da tela toda. position:
     fixed cobrindo a viewport; o backing solido evita o glitch preto do iOS. */
  .tm-backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    flex-direction: column;
    background: var(--bg-base);
  }
  .tm-head {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border-subtle);
    padding-top: max(var(--space-2), env(safe-area-inset-top));
  }
  .tm-title { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); }
  /* "Voltar ao chat" = saida SEGURA e obvia (so esconde o espelho, nao mexe na TUI). Destacado em
     accent pra nao confundir com a tecla Esc da barra (que SIM fecha o overlay na TUI). */
  .tm-back {
    display: inline-flex; align-items: center; gap: var(--space-1);
    background: var(--accent-soft, rgba(124, 147, 255, 0.16));
    border: 1px solid var(--accent);
    color: var(--accent);
    font-size: var(--text-sm); font-weight: 600;
    padding: var(--space-1) var(--space-3);
    border-radius: var(--radius-full, 999px);
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }
  .tm-back:active { background: var(--bg-hover); }
  .tm-back-arrow { font-size: var(--text-base); line-height: 1; }

  .tm-screen { flex: 1; overflow: auto; -webkit-overflow-scrolling: touch; }
  .tm-err { color: var(--danger, #f87171); font-size: var(--text-xs); padding: var(--space-2) var(--space-3); margin: 0; }
  .tm-pane {
    margin: 0;
    padding: var(--space-2);
    font-family: var(--font-mono);
    /* pequeno o bastante pra caber ~80 cols num celular; o overflow-x cobre o resto. */
    font-size: 10px;
    line-height: 1.35;
    color: var(--text-primary);
    white-space: pre;            /* sem reflow: preserva o layout do TUI */
    min-width: max-content;      /* deixa rolar horizontal em vez de quebrar */
    tab-size: 2;
  }

  .tm-keys {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    padding-bottom: max(var(--space-2), env(safe-area-inset-bottom));
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-elevated, var(--bg-base));
    overflow-x: auto;
  }
  .tm-keys-hint {
    flex-shrink: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .tm-arrows { display: flex; gap: var(--space-1); flex: 1; justify-content: center; }
  .tm-key {
    flex-shrink: 0;
    min-width: 40px;
    height: 40px;
    padding: 0 var(--space-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md, 8px);
    background: var(--bg-surface, rgba(255, 255, 255, 0.06));
    color: var(--text-primary);
    font-size: var(--text-base);
    font-family: var(--font-mono);
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }
  .tm-key:active { background: var(--accent-soft, rgba(255, 255, 255, 0.16)); }
  .tm-enter { color: var(--accent, #7c93ff); }
</style>
