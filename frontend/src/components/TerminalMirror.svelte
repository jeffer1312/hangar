<script lang="ts">
  import { getPane, sendKey, sendTermInput, type NavKey } from '../lib/api';
  import ModalDialog from './ModalDialog.svelte';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  let text = $state('');
  let busy = $state(false);
  let err = $state<string | null>(null);

  // Quanto scrollback pedir. SÓ adianta quando o tmux tem histórico de verdade — num TUI de tela
  // alternada (Claude Code) `alternate_on=1` zera o history_size e pedir mais nunca traz nada. A TUI
  // do Codex sobe com --no-alt-screen, então ali existe. `scrollback` (do backend) diz qual é o caso.
  const LINES_STEP = 400;
  const LINES_MAX = 5000;
  let paneLines = $state(200);
  let loadingMore = $state(false);
  let scrollback = $state(0);
  const canLoadMore = $derived(scrollback > 0 && paneLines < LINES_MAX);

  // Rolar pra cima era IMPOSSÍVEL na prática: o poll de 450ms trocava o conteúdo inteiro embaixo do
  // dedo. O scroll (overflow:auto) sempre existiu — o que faltava era o texto parar de se mexer.
  // Longe do fim => congela a atualização e mostra um aviso; o usuário lê em paz e volta quando quer.
  const BOTTOM_SLOP = 24;   // px de folga: "quase no fim" conta como fim
  let atBottom = $state(true);
  let pending = $state(false);   // houve mudança no pane enquanto estava congelado

  function onScroll() {
    if (!paneEl) return;
    const dist = paneEl.scrollHeight - paneEl.scrollTop - paneEl.clientHeight;
    const now = dist <= BOTTOM_SLOP;
    if (now && !atBottom) pending = false;   // voltou pro fim: nada mais atrasado
    atBottom = now;
  }

  function toBottom() {
    if (!paneEl) return;
    paneEl.scrollTop = paneEl.scrollHeight;
    atBottom = true;
    pending = false;
  }

  // Poll do pane CRU enquanto aberto. O overlay so-TUI nao gera evento no .jsonl/SSE, entao a unica
  // fonte viva e o capture-pane. ~450ms = responsivo sem martelar o backend (1 subprocess por poll).
  $effect(() => {
    if (!open) return;
    let alive = true;
    async function tick() {
      try {
        const p = await getPane(sessionName, paneLines);
        if (!alive) return;
        err = null;
        scrollback = p.scrollback;
        const t = p.text;
        if (t === text) return;                 // diff-gate: sem mudança, sem re-render
        if (!atBottom) { pending = true; return; }  // lendo o histórico: não puxa o tapete
        text = t;
        // Colado no fim = acompanha a saída nova, como um terminal de verdade.
        requestAnimationFrame(() => { if (atBottom) toBottom(); });
      } catch (e) {
        if (alive) err = e instanceof Error ? e.message : 'erro';
      }
    }
    tick();
    const id = setInterval(tick, 450);
    return () => { alive = false; clearInterval(id); };
  });

  async function loadMore() {
    if (loadingMore || paneLines >= LINES_MAX) return;
    loadingMore = true;
    const before = paneEl?.scrollHeight ?? 0;
    try {
      paneLines = Math.min(paneLines + LINES_STEP, LINES_MAX);
      text = (await getPane(sessionName, paneLines)).text;
      // Ancora no conteúdo que o usuário já estava lendo: sem isto, as linhas antigas entram por
      // cima e a posição salta pro topo do histórico novo.
      requestAnimationFrame(() => {
        if (paneEl) paneEl.scrollTop += paneEl.scrollHeight - before;
      });
    } catch (e) {
      err = e instanceof Error ? e.message : 'erro';
    } finally {
      loadingMore = false;
    }
  }

  async function press(key: NavKey) {
    if (busy) return;
    busy = true;
    try {
      await sendKey(sessionName, key);
      // Refresh imediato pra feedback instantaneo (nao espera o proximo tick do poll). Apertar
      // tecla e acao deliberada -> volta pro fim, que e onde o efeito dela aparece.
      text = (await getPane(sessionName, paneLines)).text;
      requestAnimationFrame(toBottom);
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
      text = (await getPane(sessionName, paneLines)).text;   // refresh imediato
      requestAnimationFrame(toBottom);
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

  // ── Teclado no MOBILE ───────────────────────────────────────────────────────
  // O desktop captura o teclado direto no pane; o celular não tem como, então precisava de um campo
  // real pra o teclado do sistema aparecer. Um POST por tecla seria conversa demais em rede móvel —
  // e o /term-input já aceita texto E tecla no MESMO corpo (manda o texto, depois a tecla), então
  // "digitar e enviar" é uma requisição só, sem corrida entre as duas partes.
  let draft = $state('');
  let sending = $state(false);

  // ── Tamanho da fonte ────────────────────────────────────────────────────────
  // 10px fixo era ilegível no celular; num pane de 200 colunas o tamanho é o único controle real
  // de quanto cabe na tela. Persistido: reajustar isso a cada abertura seria atrito puro.
  const FONT_MIN = 8, FONT_MAX = 18, FONT_KEY = 'cp_mirror_font';
  let fontPx = $state(
    (() => {
      const v = Number(typeof localStorage !== 'undefined' ? localStorage.getItem(FONT_KEY) : NaN);
      return Number.isFinite(v) && v >= FONT_MIN && v <= FONT_MAX ? v : 11;
    })(),
  );
  function bumpFont(d: number) {
    fontPx = Math.min(FONT_MAX, Math.max(FONT_MIN, fontPx + d));
    try { localStorage.setItem(FONT_KEY, String(fontPx)); } catch { /* modo privado */ }
  }

  async function submitDraft(withEnter: boolean) {
    const value = draft;
    if (!value || sending) return;
    sending = true;
    try {
      await sendInput(withEnter ? { text: value, key: 'Enter' } : { text: value });
      draft = '';
    } finally {
      sending = false;
    }
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

<ModalDialog {open} ariaLabel="Terminal (overlay TUI)" onClose={onClose} className="tm-dialog">
  <div class="tm-backdrop">
    <header class="tm-head">
      <button class="tm-back" onclick={onClose} aria-label="Voltar ao chat">
        <span class="tm-back-arrow">←</span> Voltar ao chat
      </button>
      <div class="tm-head-right">
        <div class="tm-font" role="group" aria-label="Tamanho da fonte">
          <button class="tm-fontbtn" onclick={() => bumpFont(-1)} disabled={fontPx <= FONT_MIN}
                  aria-label="Diminuir fonte">A−</button>
          <span class="tm-fontval">{fontPx}</span>
          <button class="tm-fontbtn" onclick={() => bumpFont(1)} disabled={fontPx >= FONT_MAX}
                  aria-label="Aumentar fonte">A+</button>
        </div>
        <span class="tm-title">{sessionName}{#if interactive} · <span class="tm-live">interativo</span>{/if}</span>
      </div>
    </header>

    <div
      class="tm-screen"
      class:interactive
      bind:this={paneEl}
      onscroll={onScroll}
      tabindex={interactive ? 0 : undefined}
      role="textbox"
      aria-readonly={!interactive}
      aria-label={interactive ? 'Terminal interativo — digite' : undefined}
      onkeydown={interactive ? onTermKey : undefined}
    >
      {#if err}
        <p class="tm-err">{err}</p>
      {/if}
      {#if canLoadMore}
        <button class="tm-more" onclick={loadMore} disabled={loadingMore}>
          {loadingMore ? 'carregando…' : `↑ carregar mais histórico (${paneLines} linhas)`}
        </button>
      {/if}
      <pre class="tm-pane" style:font-size={`${fontPx}px`}>{lines.join('\n')}</pre>
    </div>

    {#if !atBottom}
      <button class="tm-frozen" onclick={toBottom}>
        {pending ? '⏸ pausado — há saída nova' : '⏸ pausado'} · ↓ voltar ao fim
      </button>
    {/if}

    {#if paneUrl}
      <a class="tm-link" href={paneUrl} target="_blank" rel="noopener noreferrer">↗ Abrir link no navegador</a>
    {/if}

    {#if !interactive}
      <form class="tm-compose" onsubmit={(e) => { e.preventDefault(); submitDraft(true); }}>
        <input
          class="tm-input"
          bind:value={draft}
          placeholder="digitar no terminal…"
          aria-label="Texto para o terminal"
          autocapitalize="off"
          autocorrect="off"
          spellcheck="false"
          enterkeyhint="send"
        />
        <!-- Enviar SEM Enter: num picker/filtro o Enter submeteria antes da hora. -->
        <button class="tm-key" type="button" disabled={!draft || sending}
                onclick={() => submitDraft(false)} title="enviar sem Enter">↵̸</button>
        <button class="tm-key tm-enter" type="submit" disabled={!draft || sending}
                title="enviar e pressionar Enter">envia ⏎</button>
      </form>
    {/if}

    <nav class="tm-keys" aria-label="Teclas de resgate">
      <span class="tm-keys-hint">resgate</span>
      <button class="tm-key" onclick={() => press('Escape')}>Esc</button>
      <button class="tm-key" onclick={() => press('Tab')}>⇥</button>
      <!-- Rolar o PRÓPRIO TUI. Num app de tela alternada (Claude Code) o tmux não guarda scrollback,
           então rolar o espelho só percorre as ~45 linhas da tela; quem sobe no histórico é o TUI.
           Medido: PageUp muda o conteúdo do pane. Já estavam na allowlist do backend, faltava botão. -->
      <button class="tm-key" onclick={() => press('PageUp')} title="rolar o terminal para cima">⇞</button>
      <button class="tm-key" onclick={() => press('PageDown')} title="rolar o terminal para baixo">⇟</button>
      <div class="tm-arrows">
        <button class="tm-key" onclick={() => press('Left')}>←</button>
        <button class="tm-key" onclick={() => press('Up')}>↑</button>
        <button class="tm-key" onclick={() => press('Down')}>↓</button>
        <button class="tm-key" onclick={() => press('Right')}>→</button>
      </div>
      <button class="tm-key tm-enter" onclick={() => press('Enter')}>⏎</button>
    </nav>
  </div>
</ModalDialog>

<style>
  /* Overlay fullscreen (NAO bottom sheet): o pane e largo (200 cols) e precisa da tela toda. position:
     fixed cobrindo a viewport; o backing solido evita o glitch preto do iOS. */
  :global(.tm-dialog) { width: 100%; max-width: 100%; height: 100%; max-height: 100%; padding: 0; border: 0; border-radius: 0; }
  .tm-backdrop {
    height: 100%;
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
  .tm-title { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 40vw; }
  .tm-head-right { display: flex; align-items: center; gap: var(--space-3); min-width: 0; }
  .tm-font { display: flex; align-items: center; gap: var(--space-1); }
  .tm-fontbtn {
    min-width: 30px; height: 28px;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md, 8px);
    background: var(--bg-surface, rgba(255, 255, 255, 0.06));
    color: var(--text-secondary);
    font-size: var(--text-xs); font-family: var(--font-mono);
    cursor: pointer; -webkit-tap-highlight-color: transparent;
  }
  .tm-fontbtn:disabled { opacity: 0.4; }
  .tm-fontval { font-family: var(--font-mono); font-size: 10px; color: var(--text-muted); min-width: 14px; text-align: center; }
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

  /* Superfície do terminal: um tom abaixo do chrome do app, como a janela de um emulador. Sem
     backdrop-filter/transform aqui — no iOS eles promovem camada e pintam preto no scroll. */
  .tm-screen {
    flex: 1;
    overflow: auto;
    -webkit-overflow-scrolling: touch;
    background: var(--bg-surface);
  }
  /* Interativo (desktop): focavel -> anel accent discreto, sinaliza que o teclado vai pro tmux. */
  .tm-screen.interactive { outline: none; cursor: text; }
  .tm-screen.interactive:focus-visible { box-shadow: inset 0 0 0 2px var(--accent); }
  .tm-live { color: var(--accent); font-weight: 600; }
  .tm-err { color: var(--danger, #f87171); font-size: var(--text-xs); padding: var(--space-2) var(--space-3); margin: 0; }
  .tm-pane {
    margin: 0;
    /* Respiro tipo emulador de terminal: o texto não encosta na borda da tela. */
    padding: var(--space-3) var(--space-3) var(--space-4);
    font-family: var(--font-mono);
    /* font-size vem inline (controle A−/A+, persistido) — o pane tem 200 colunas e o tamanho é o
       único controle real de quanto cabe. line-height mais folgado: o TUI usa moldura de caixa
       (│ ╭ ╰) e apertado demais elas se tocam e viram borrão. */
    line-height: 1.45;
    color: var(--text-primary);
    white-space: pre;            /* sem reflow: preserva o layout do TUI */
    min-width: max-content;      /* deixa rolar horizontal em vez de quebrar */
    tab-size: 2;
    font-variant-ligatures: none;  /* ligadura em -> / != desalinha a grade de colunas */
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

  /* Botao de carregar scrollback: fica no TOPO do conteudo, alcance natural de quem rolou pra cima. */
  .tm-more {
    display: block;
    width: 100%;
    padding: var(--space-2);
    border: 0;
    border-bottom: 1px solid var(--border-subtle);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-family: var(--font-mono);
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }
  .tm-more:active { background: var(--bg-hover); }
  .tm-more:disabled { opacity: 0.6; }

  /* Aviso de congelado: o pane parou de atualizar porque o usuario esta lendo o historico. Sem
     isto o congelamento pareceria a tela travada. Toque volta pro fim e retoma. */
  .tm-frozen {
    flex-shrink: 0;
    margin: 0 var(--space-3) var(--space-2);
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--accent);
    border-radius: var(--radius-md, 8px);
    background: var(--accent-soft, rgba(124, 147, 255, 0.16));
    color: var(--accent);
    font-size: var(--text-xs);
    font-weight: 600;
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }

  /* Campo de digitação (mobile). Acima da barra de teclas pra o teclado do sistema não cobrir os
     dois; ambos flex-shrink:0 pra o pane ceder altura, nunca eles. */
  .tm-compose {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-top: 1px solid var(--border-subtle);
  }
  .tm-input {
    flex: 1;
    min-width: 0;
    height: 40px;
    padding: 0 var(--space-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md, 8px);
    background: var(--bg-surface, rgba(255, 255, 255, 0.06));
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 16px;   /* < 16px faz o iOS dar zoom no foco */
  }
  .tm-input:focus { outline: none; border-color: var(--accent); }
  .tm-compose .tm-key:disabled { opacity: 0.45; }

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
