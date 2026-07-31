<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import AppearanceSettings from './settings/AppearanceSettings.svelte';

  interface Props { open: boolean; onClose: () => void }
  let { open, onClose }: Props = $props();

  // Breakpoint desktop, reativo (nao um retrato do boot): atravessar 820px troca o formato na hora.
  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });
</script>

<!-- No desktop vira MODAL centrado e largo, nao dock estreito: sao 8 secoes de rotulo+controle, e
     num painel de ~530px a descricao de cada uma disputava a linha com o segmentado e quebrava em
     palavras soltas. Mesmo par `wide`+`centered` do EnginesSheet e do modal de git. -->
<BottomSheet {open} {onClose} ariaLabel="Aparência" wide={isDesktop} centered={isDesktop}>
  <h2 class="sheet-title">Aparência</h2>
  <AppearanceSettings />
</BottomSheet>

<style>
  .sheet-title {
    margin: 0 0 var(--space-4);
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
  }
</style>
