<script lang="ts">
  import { getPane, sendKey, sendTermInput, type NavKey } from '../lib/api';

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

  // ── Terminal INTERATIVO (so desktop): digitar direto no pane -> tmux ────────
  // Mobile fica read-only de proposito (barra de teclas de resgate). Desktop (ponteiro fino) captura
  // o teclado e manda pro backend (/term-input): texto literal + control-chars de shell/TUI.
  const interactive = typeof window !== 'undefined'
    && window.matchMedia('(hover: hover) and (pointer: fine)').matches;
  let paneEl: HTMLElement | undefined = $state();

  const NAMED: Record<string, string> = {
    Enter: 'Enter', Backspace: 'Backspace', Tab: 'Tab', Escape: 'Escape',
    ArrowUp: 'Up', ArrowDown: 'Down', ArrowLeft: 'Left', ArrowRight: 'Right',
    Delete: 'Delete', Home: 'Home', End: 'End', PageUp: 'PageUp', PageDown: 'PageDown',
  };

  async function sendInput(payload: { text?: string; key?: string }) {
    try {
      await sendTermInput(sessionName, payload);
      text = await getPane(sessionName);   // refresh imediato
    } catch (e) {
      err = e instanceof Error ? e.message : 'erro';
    }
  }

  async function onTermKey(e: KeyboardEvent) {
    // stopPropagation: nao deixa o Esc/atalhos do Chat (fechar overlay, Cmd+K) roubarem a tecla.
    if (e.ctrlKey && !e.altKey && !e.metaKey && e.key.length === 1) {
      e.preventDefault(); e.stopPropagation();
      await sendInput({ key: 'C-' + e.key.toLowerCase() });   // Ctrl+C, Ctrl+R, Ctrl+D...
      return;
    }
    if (e.altKey || e.metaKey) return;   // atalhos do SO/navegador passam
    const named = NAMED[e.key];
    if (named) { e.preventDefault(); e.stopPropagation(); await sendInput({ key: named }); return; }
    if (e.key.length === 1) { e.preventDefault(); e.stopPropagation(); await sendInput({ text: e.key }); }
  }

  // Foca o pane ao abrir no desktop -> digita direto sem clicar.
  $effect(() => { if (open && interactive) setTimeout(() => paneEl?.focus(), 60); });

  // Linhas de chrome do rodape (statusline + box de input) sao ruido aqui — mas mantemos o pane
  // INTEIRO pra nao esconder nada do overlay. Trim so as linhas vazias do fim.
  const lines = $derived(text.replace(/\s+$/, '').split('\n'));

  // URL no pane (ex: link OAuth do login). O pane tem 200 cols, entao a URL cabe numa linha so e o
  // match pega ela inteira. Vira botao tocavel -> abre no browser do celular (copiar link gigante de
  // uma fonte 10px era o atrito no login). Primeira URL basta; null some o botao.
  const paneUrl = $derived(text.match(/https?:\/\/\S+/)?.[0] ?? null);
</script>

{#if open}
  <div class="tm-backdrop" role="dialog" aria-modal="true" aria-label="Terminal (overlay TUI)">
    <header class="tm-head">
      <button class="tm-back" onclick={onClose} aria-label="Voltar ao chat">
        <span class="tm-back-arrow">←</span> Voltar ao chat
      </button>
      <span class="tm-title">⌨ {sessionName}{#if interactive} · <span class="tm-live">interativo</span>{/if}</span>
    </header>

    <!-- svelte-ignore a11y_no_static_element_interactions a11y_no_noninteractive_tabindex -->
    <div
      class="tm-screen"
      class:interactive
      bind:this={paneEl}
      tabindex={interactive ? 0 : undefined}
      role={interactive ? 'textbox' : undefined}
      aria-label={interactive ? 'Terminal interativo — digite' : undefined}
      onkeydown={interactive ? onTermKey : undefined}
    >
      {#if err}
        <p class="tm-err">{err}</p>
      {/if}
      <pre class="tm-pane">{lines.join('\n')}</pre>
    </div>

    {#if paneUrl}
      <a class="tm-link" href={paneUrl} target="_blank" rel="noopener noreferrer">↗ Abrir link no navegador</a>
    {/if}

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
  /* Interativo (desktop): focavel -> anel accent discreto, sinaliza que o teclado vai pro tmux. */
  .tm-screen.interactive { outline: none; cursor: text; }
  .tm-screen.interactive:focus-visible { box-shadow: inset 0 0 0 2px var(--accent); }
  .tm-live { color: var(--accent); font-weight: 600; }
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

  /* Botao tocavel quando ha URL no pane (link OAuth do login). Largo e obvio — vs a URL crua 10px. */
  .tm-link {
    flex-shrink: 0;
    display: block;
    text-align: center;
    margin: 0 var(--space-3) var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--accent-soft, rgba(124, 147, 255, 0.16));
    border: 1px solid var(--accent);
    border-radius: var(--radius-md, 8px);
    color: var(--accent);
    font-size: var(--text-sm);
    font-weight: 600;
    text-decoration: none;
    word-break: break-all;
    -webkit-tap-highlight-color: transparent;
  }
  .tm-link:active { background: var(--bg-hover); }

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
