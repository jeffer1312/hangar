<script lang="ts">
  import { TermSocket, termUrl } from '../lib/term';

  interface Props { sessionName: string; open: boolean; onClose: () => void; }
  let { sessionName, open, onClose }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let maximizado = $state(false);
  let caiu = $state(false);
  let geracao = $state(0);          // incrementar reconecta de verdade
  let sock: TermSocket | null = null;
  let term: any = null;
  let fit: any = null;
  let ro: ResizeObserver | null = null;

  $effect(() => {
    // Os tres lidos AQUI, sincronos: se `sessionName` so fosse lido dentro do callback async (depois
    // do await), o Svelte nao o rastrearia e trocar de sessao no sidebar deixaria o terminal preso
    // na anterior. `geracao` e o que faz o botao de reconectar funcionar.
    const alvo = sessionName;
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
      // glifo errada e grade desalinhada.
      const mono = getComputedStyle(host).getPropertyValue('--font-mono').trim() || 'monospace';

      term = new Terminal({ fontFamily: mono, fontSize: 12, convertEol: false });
      fit = new FitAddon();
      term.loadAddon(fit);
      term.open(host);
      fit.fit();

      const enc = new TextEncoder();
      sock = new TermSocket(termUrl(alvo, term.cols, term.rows), {
        data: (b) => term.write(b),
        close: () => { caiu = true; },
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
  <section class="tp" class:max={maximizado}
           onkeydowncapture={(e) => { if (e.key === 'Escape') { e.stopPropagation(); onClose(); } else e.stopPropagation(); }}>
    <header class="tp-bar">
      <span class="tp-nome">{sessionName}</span>
      {#if caiu}
        <button class="tp-recon" onclick={() => geracao++}>desconectado · reconectar</button>
      {/if}
      <button onclick={() => (maximizado = !maximizado)} aria-label="Maximizar">⤢</button>
      <button onclick={onClose} aria-label="Fechar">✕</button>
    </header>
    <div class="tp-screen" bind:this={host}></div>
  </section>
{/if}

<style>
  .tp { display: flex; flex-direction: column; height: 320px; border-top: 1px solid var(--border-subtle); resize: vertical; overflow: hidden; }
  .tp.max { position: absolute; inset: 0; height: auto; z-index: 5; }
  /* transparent de proposito: quem carrega o material e o conteiner (regra de vidro do CLAUDE.md). */
  .tp-bar { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) var(--space-2); background: transparent; }
  .tp-nome { flex: 1; font-size: var(--text-xs); color: var(--text-muted); }
  .tp-screen { flex: 1; min-height: 0; background: var(--surface-inset); }
</style>
