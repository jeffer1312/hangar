<script lang="ts">
  import { TermSocket, termUrl } from '../lib/term';

  interface Props { sessionName: string; connKey: string; open: boolean; onClose: () => void; }
  let { sessionName, connKey, open, onClose }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let secEl = $state<HTMLElement | null>(null);
  let maximizado = $state(false);
  let caiu = $state(false);
  let geracao = $state(0);          // incrementar reconecta de verdade
  let sock: TermSocket | null = null;
  let term: any = null;
  let fit: any = null;
  let ro: ResizeObserver | null = null;

  // Fecha (o X) reseta o maximizado: sem isto o painel reabria maximizado por acidente, herdando
  // estado da vez anterior.
  $effect(() => { if (!open) maximizado = false; });

  function toggleMax() {
    maximizado = !maximizado;
    // `resize: vertical` grava `height` INLINE no elemento ao arrastar a borda; inline vence o
    // `height: auto` de `.tp.max` no cascade, entao maximizar depois de arrastar nao maximizava.
    if (maximizado && secEl) secEl.style.height = '';
  }

  $effect(() => {
    // Os tres lidos AQUI, sincronos: se `sessionName` so fosse lido dentro do callback async (depois
    // do await), o Svelte nao o rastrearia e trocar de sessao no sidebar deixaria o terminal preso
    // na anterior. `geracao` e o que faz o botao de reconectar funcionar.
    const alvo = sessionName;
    // connKey (server-aware, "servidor::nome"): so o nome nao bastava — trocar de servidor com uma
    // sessao homonima na tela nao mudava `sessionName`, e o socket ficava conectado no servidor
    // VELHO (termUrl usa o servidor ATIVO no instante da conexao, nao um serverId explicito).
    void connKey;
    void geracao;
    if (!open || !host) return;
    let vivo = true;
    caiu = false;

    (async () => {
      // Import DINAMICO: xterm so entra no bundle de quem abre o terminal. E feature desktop-only na
      // v1 — o PWA do celular nao pode pagar o download.
      const [{ Terminal }, { FitAddon }] = await Promise.all([
        import('@xterm/xterm'),
        import('@xterm/addon-fit'),
      ]);
      await import('@xterm/xterm/css/xterm.css');
      if (!vivo || !host) return;

      // getComputedStyle, nao `var(--font-mono)` cru: o renderer canvas monta
      // `ctx.font = \`${size}px ${family}\``, onde var() e invalido e ignorado calado -> metrica de
      // glifo errada e grade desalinhada. Mesma razao pras cores do tema logo abaixo.
      const cs = getComputedStyle(host);
      const mono = cs.getPropertyValue('--font-mono').trim() || 'monospace';
      const fg = cs.getPropertyValue('--text-primary').trim() || '#d2cbcd';
      const cursor = cs.getPropertyValue('--accent').trim() || fg;

      term = new Terminal({
        fontFamily: mono, fontSize: 12, convertEol: false,
        // allowTransparency + background transparente: sem isto o xterm pinta #000 opaco por cima
        // do --surface-inset do .tp-screen (regra de vidro do CLAUDE.md) — retangulo preto chapado
        // sobre o papel de parede, e caixa preta crua no tema claro.
        allowTransparency: true,
        theme: { background: 'transparent', foreground: fg, cursor },
      });
      fit = new FitAddon();
      term.loadAddon(fit);
      term.open(host);
      fit.fit();

      const enc = new TextEncoder();
      sock = new TermSocket(termUrl(alvo, term.cols, term.rows), {
        data: (b) => term.write(b),
        // `vivo`, nao incondicional: TermSocket.close() dispara onclose ASSINCRONO. Ao trocar de
        // sessao, o cleanup fecha o socket velho -> o efeito novo zera `caiu` -> DEPOIS chega o
        // onclose do socket velho -> sem a guarda, `caiu = true` aterrissava na sessao ERRADA (ou
        // num componente ja destruido, se foi o painel que fechou).
        close: () => { if (vivo) caiu = true; },
      });
      term.onData((d: string) => sock?.send(enc.encode(d)));

      ro = new ResizeObserver(() => { fit?.fit(); sock?.resize(term.cols, term.rows); });
      ro.observe(host);
    })();

    return () => {
      vivo = false;
      ro?.disconnect(); ro = null;
      sock?.close(); sock = null;
      term?.dispose(); term = null;
    };
  });
</script>

{#if open}
  <!-- UM mount, DUAS classes: mover o componente entre conteineres re-montaria o xterm.js e
       derrubaria o socket a cada troca de tamanho. -->
  <!-- svelte-ignore a11y_no_static_element_interactions (onkeydown so pra parar propagacao; quem
       captura teclado de verdade e o textarea do proprio xterm, ja nativamente interativo) -->
  <section class="tp" class:max={maximizado} bind:this={secEl}
           onkeydown={(e) => {
             // BOLHA, nao captura: o xterm registra o keydown no proprio textarea (descendente desta
             // section) e trata a tecla PRIMEIRO na fase de captura -- se a gente parasse na captura,
             // o evento nunca chegava nele (digitar/setas/Ctrl+C morriam mudos).
             // Esc vai pro AGENTE, nao fecha o painel: e a tecla que interrompe o Claude Code na TUI,
             // e um terminal onde Esc fecha a janela em vez de chegar no Claude perde a razao de
             // existir (decisao do dono do plano). So o botao X fecha.
             // stopPropagation em QUALQUER tecla (Esc incluso) so pra nao vazar pros atalhos globais
             // do Chat (svelte:window onkeydown) enquanto o usuario digita aqui dentro.
             e.stopPropagation();
           }}>
    <header class="tp-bar">
      <span class="tp-nome">{sessionName}</span>
      {#if caiu}
        <button class="tp-recon" onclick={() => geracao++}>desconectado · reconectar</button>
      {/if}
      <button onclick={toggleMax} aria-label="Maximizar">⤢</button>
      <button onclick={onClose} aria-label="Fechar">✕</button>
    </header>
    <div class="tp-screen" bind:this={host}></div>
  </section>
{/if}

<style>
  .tp { display: flex; flex-direction: column; height: 320px; border-top: 1px solid var(--border-subtle); resize: vertical; overflow: hidden; }
  /* z-index 40, nao 5: precisa cobrir o .board-overlay (DesktopShell.svelte, z-index:30) quando o
     terminal e aberto de dentro do chat-overlay do board/canvas e depois maximizado. */
  .tp.max { position: absolute; inset: 0; height: auto; z-index: 40; }
  /* transparent de proposito: quem carrega o material e o conteiner (regra de vidro do CLAUDE.md). */
  .tp-bar { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) var(--space-2); background: transparent; }
  .tp-nome { flex: 1; font-size: var(--text-xs); color: var(--text-muted); }
  .tp-screen { flex: 1; min-height: 0; background: var(--surface-inset); }
</style>
