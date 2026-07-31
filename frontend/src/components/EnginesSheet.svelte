<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import EnginesSettings from './settings/EnginesSettings.svelte';
  import type { Server } from '../lib/auth';

  interface Props {
    open: boolean;
    onClose: () => void;
    targetServer?: Server | null;
  }
  let { open, onClose, targetServer = null }: Props = $props();

  // Breakpoint desktop (mesmo corte do resto do app). Acima dele a folha vira modal largo: o
  // Avançado são 9 controles e num painel de 365px eles viram corredor de texto.
  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });
</script>

<BottomSheet {open} {onClose} ariaLabel="Motores de modelo" wide={isDesktop} centered={isDesktop}>
  <EnginesSettings {targetServer} />
</BottomSheet>
