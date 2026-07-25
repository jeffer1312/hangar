<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type QrScanner from 'qr-scanner';
  import ModalDialog from './ModalDialog.svelte';

  interface Props {
    onScan: (text: string) => void;
    onClose: () => void;
  }
  let { onScan, onClose }: Props = $props();

  let videoEl: HTMLVideoElement | undefined = $state();
  let scanner: QrScanner | null = null;
  let error = $state('');
  let dead = false;
  let dialogOpen = $state(true);

  function stopScanner() {
    scanner?.stop();
    scanner?.destroy();
    scanner = null;
  }

  function closeScanner() {
    stopScanner();
    dialogOpen = false;
    // Let ModalDialog run its focus-restore effect before the parent unmounts us.
    setTimeout(onClose, 0);
  }

  onMount(async () => {
    if (!videoEl) return;
    // qr-scanner fora do bundle inicial: so carrega quando o scanner abre (pareamento e raro).
    const { default: Scanner } = await import('qr-scanner');
    if (dead || !videoEl) return; // fechou enquanto o chunk carregava
    scanner = new Scanner(
      videoEl,
      (result) => {
        stopScanner(); // first hit wins — stop before handing off
        dialogOpen = false;
        // Restore focus only after the camera is stopped and the modal has closed.
        setTimeout(() => onScan(result.data), 0);
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
    stopScanner();
  });
</script>

<ModalDialog open={dialogOpen} ariaLabel="Escanear QR" closeOnBackdrop={false} className="scanner-dialog" onClose={closeScanner}>
<div class="scanner">
  <div class="scanner-stage">
    <!-- svelte-ignore a11y_media_has_caption -->
    <video bind:this={videoEl} class="scanner-video" playsinline muted></video>
  </div>

  {#if error}
    <p class="scanner-error" role="alert">{error}</p>
  {:else}
    <p class="scanner-hint">Aponte para o QR do terminal</p>
  {/if}

  <button class="scanner-close" onclick={closeScanner}>Cancelar</button>
</div>
</ModalDialog>

<style>
  :global(.scanner-dialog) { width: 100%; max-width: 100%; height: 100%; max-height: 100%; padding: 0; border: 0; border-radius: 0; background: #000; }
  .scanner {
    height: 100%;
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
