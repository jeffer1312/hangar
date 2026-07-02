<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type QrScanner from 'qr-scanner';

  interface Props {
    onScan: (text: string) => void;
    onClose: () => void;
  }
  let { onScan, onClose }: Props = $props();

  let videoEl: HTMLVideoElement | undefined = $state();
  let scanner: QrScanner | null = null;
  let error = $state('');
  let dead = false;

  onMount(async () => {
    if (!videoEl) return;
    // qr-scanner fora do bundle inicial: so carrega quando o scanner abre (pareamento e raro).
    const { default: Scanner } = await import('qr-scanner');
    if (dead || !videoEl) return; // fechou enquanto o chunk carregava
    scanner = new Scanner(
      videoEl,
      (result) => {
        scanner?.stop(); // first hit wins — stop before handing off
        onScan(result.data);
      },
      { preferredCamera: 'environment', highlightScanRegion: true, highlightCodeOutline: true },
    );
    try {
      await scanner.start();
    } catch {
      error = 'Não consegui abrir a câmera. Permita o acesso (precisa de HTTPS).';
    }
  });

  onDestroy(() => {
    dead = true;
    scanner?.stop();
    scanner?.destroy();
    scanner = null;
  });
</script>

<div class="scanner" role="dialog" aria-label="Escanear QR">
  <div class="scanner-stage">
    <!-- svelte-ignore a11y_media_has_caption -->
    <video bind:this={videoEl} class="scanner-video" playsinline muted></video>
  </div>

  {#if error}
    <p class="scanner-error" role="alert">{error}</p>
  {:else}
    <p class="scanner-hint">Aponte para o QR do terminal</p>
  {/if}

  <button class="scanner-close" onclick={onClose}>Cancelar</button>
</div>

<style>
  .scanner {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: #000;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-5);
    padding: var(--space-6);
    padding-top: calc(env(safe-area-inset-top) + var(--space-6));
    padding-bottom: calc(env(safe-area-inset-bottom) + var(--space-6));
  }

  .scanner-stage {
    width: 100%;
    max-width: 360px;
    aspect-ratio: 1;
    border-radius: var(--radius-lg);
    overflow: hidden;
    background: #111;
  }

  .scanner-video {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .scanner-hint {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    text-align: center;
  }

  .scanner-error {
    font-size: var(--text-sm);
    color: var(--error);
    text-align: center;
  }

  .scanner-close {
    height: 48px;
    padding: 0 var(--space-8);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-size: var(--text-base);
  }
</style>
