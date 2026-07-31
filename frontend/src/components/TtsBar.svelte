<script lang="ts">
  import { ttsPlayer } from '../lib/ttsPlayer.svelte';
  import { formatClock } from '../lib/ttsFormat';

  const RATES = [1, 1.25, 1.5];

  function proximaVelocidade() {
    const i = RATES.indexOf(ttsPlayer.rate);
    ttsPlayer.setRate(RATES[(i + 1) % RATES.length]);
  }

  let barEl = $state<HTMLDivElement | null>(null);

  // Mede a propria altura e publica no ttsPlayer (App.svelte compoe --cp-tts-bar-h/--cp-tts-h a
  // partir dai) — nao e mais um 52px cravado: o erro real da ElevenLabs quebra em 2-3 linhas num
  // celular estreito e passa longe disso. Mesmo padrao do dockH em Chat.svelte. Some da tela
  // (ttsPlayer.active vira false) -> barEl volta null -> publica 0, senao a TtsSelectionPill e as
  // tres pills do Chat.svelte ficariam flutuando alto pra sempre depois do 1o audio.
  $effect(() => {
    if (!barEl) { ttsPlayer.setBarH(0); return; }
    const el = barEl;
    let raf = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        ttsPlayer.setBarH(Math.round(el.getBoundingClientRect().height));
      });
    });
    ro.observe(el);
    return () => { cancelAnimationFrame(raf); ro.disconnect(); ttsPlayer.setBarH(0); };
  });
</script>

{#if ttsPlayer.active}
  <div class="tts-bar" bind:this={barEl} role="region" aria-label="Leitura em voz">
    {#if ttsPlayer.error}
      <span class="tts-err">{ttsPlayer.error}</span>
    {:else if ttsPlayer.loading}
      <span class="tts-load">gerando áudio…</span>
    {:else}
      <button class="tts-play" onclick={() => ttsPlayer.toggle()}
              aria-label={ttsPlayer.playing ? 'Pausar' : 'Tocar'}>
        {ttsPlayer.playing ? '⏸' : '▶'}
      </button>
      <input class="tts-seek" type="range" min="0" step="0.1"
             max={Number.isFinite(ttsPlayer.duration) && ttsPlayer.duration > 0 ? ttsPlayer.duration : 0}
             value={ttsPlayer.current}
             oninput={(e) => ttsPlayer.seek(Number((e.currentTarget as HTMLInputElement).value))}
             aria-label="Posição" />
      <span class="tts-time">{formatClock(ttsPlayer.current)}/{formatClock(ttsPlayer.duration)}</span>
      <button class="tts-rate" onclick={proximaVelocidade} aria-label="Velocidade">{ttsPlayer.rate}×</button>
    {/if}
    <button class="tts-close" onclick={() => ttsPlayer.close()} aria-label="Fechar">✕</button>
  </div>
{/if}

<style>
  /* Fixa acima do composer. Sem backdrop-filter/transform: no WebKit isso promove camada e pinta
     retangulo preto durante a rolagem por inercia. */
  .tts-bar {
    position: fixed;
    left: 50%;
    transform: none;
    bottom: calc(var(--cp-dock-h, 150px) + 10px);
    /* margin-left precisa ser -metade da largura da barra: -50vw+8px no celular (largura ~100vw-16px),
       -320px no desktop (metade dos 640px do width max). Os dois numeros sao negativos, entao o
       menor MODULO (mais perto de zero) e quem da a metade certa da largura real — e isso e o
       maior dos dois, nao o menor: max(-50vw + 8px, -320px). Com min(), no celular a barra pega
       -320px (a metade do desktop) e sai cortada pela borda esquerda da tela. */
    margin-left: max(-50vw + 8px, -320px);
    width: min(calc(100vw - 16px), 640px);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 14px;
    /* superficie propria dentro de painel: acompanha o slider de transparencia */
    background: var(--surface-raised);
    border: 1px solid var(--border-subtle);
    /* 39, e nao 40: o backdrop dos menus e sheets vive em 40 (Sidebar.svelte:2005,
       SessionContextMenu.svelte:199) e o conteudo deles em 41/42. Empatar em 40 deixaria a ordem
       entre a barra e um menu aberto por conta da posicao no DOM. Abaixo do backdrop tambem e o
       certo por desenho: player nao pode cobrir modal aberto. Mesma faixa do HoverPreview. */
    z-index: 39;
  }
  .tts-seek { flex: 1; min-width: 0; accent-color: var(--accent); }
  .tts-time { font-variant-numeric: tabular-nums; font-size: 12px; color: var(--text-secondary); }
  .tts-play, .tts-rate, .tts-close {
    background: transparent;
    border: 0;
    color: var(--text-primary);
    cursor: pointer;
    padding: 4px 6px;
    font-size: 14px;
  }
  .tts-err { color: var(--error); font-size: 13px; flex: 1; }
  .tts-load { color: var(--text-secondary); font-size: 13px; flex: 1; }
</style>
