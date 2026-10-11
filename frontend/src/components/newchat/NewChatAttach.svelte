<script lang="ts">
  import BottomSheet from '../BottomSheet.svelte';
  import DitadoEstiloPopover from '../DitadoEstiloPopover.svelte';
  import * as m from '../../paraglide/messages';

  // "+" do campo do início (decisão 7 do desenho): câmera, fotos, arquivo e estilo do ditado. Os
  // arquivos ficam com quem chama e sobem na sessão nova, depois de criá-la.
  interface Props { disabled?: boolean; onFiles: (files: File[]) => void }
  let { disabled = false, onFiles }: Props = $props();

  let open = $state(false);
  let styleOpen = $state(false);
  let styleAnchor = $state<HTMLElement | null>(null);
  let camera: HTMLInputElement | undefined = $state();
  let photos: HTMLInputElement | undefined = $state();
  let file: HTMLInputElement | undefined = $state();

  function picked(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const list = [...(input.files ?? [])];
    input.value = '';
    open = false;
    if (list.length) onFiles(list);
  }
</script>

<button type="button" class="plus" aria-label={m.newchat_attach()} {disabled} onclick={() => (open = true)}>
  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
    <line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" />
  </svg>
</button>

<input bind:this={camera} type="file" accept="image/*" capture="environment" hidden onchange={picked} />
<input bind:this={photos} type="file" accept="image/*" multiple hidden onchange={picked} />
<input bind:this={file} type="file" multiple hidden onchange={picked} />

<BottomSheet {open} onClose={() => (open = false)} ariaLabel={m.newchat_attach()}>
  <ul class="list">
    <li><button type="button" class="row" onclick={() => camera?.click()}>{m.newchat_attach_camera()}</button></li>
    <li><button type="button" class="row" onclick={() => photos?.click()}>{m.newchat_attach_photos()}</button></li>
    <li><button type="button" class="row" onclick={() => file?.click()}>{m.newchat_attach_file()}</button></li>
    <li>
      <button type="button" class="row" bind:this={styleAnchor} onclick={() => (styleOpen = true)}>
        {m.newchat_attach_dictation_style()}
      </button>
    </li>
  </ul>
</BottomSheet>

<DitadoEstiloPopover open={styleOpen} anchor={styleAnchor} onClose={() => { styleOpen = false; open = false; }} />

<style>
  .plus {
    width: 44px; height: 44px; flex-shrink: 0; display: inline-flex; align-items: center; justify-content: center;
    border: 0; border-radius: var(--radius-full); background: transparent; color: var(--text-secondary); cursor: pointer;
  }
  .plus:disabled { opacity: 0.5; cursor: default; }
  .list { list-style: none; margin: 0; padding: var(--space-2) 0 calc(env(safe-area-inset-bottom) + var(--space-4)); }
  .row {
    width: 100%; min-height: 52px; display: flex; align-items: center; padding: 0 var(--space-4);
    border: 0; background: transparent; color: var(--text-primary); font-size: var(--text-base); text-align: left; cursor: pointer;
  }
</style>
