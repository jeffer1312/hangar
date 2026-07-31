<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import ServerSettings from './settings/ServerSettings.svelte';
  import type { Server } from '../lib/auth';

  interface Props {
    open: boolean;
    onClose: () => void;
    targetServer?: Server | null;
    onOpenMotores: () => void;
  }
  let { open, onClose, targetServer = null, onOpenMotores }: Props = $props();

  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });
</script>

<!-- Modal centrado e largo no desktop, pelo mesmo motivo da Aparencia: aqui sao ~12 ajustes com
     descricao, e no dock estreito o texto quebrava em palavras soltas ao lado do controle. -->
<!-- Sem `{#if open}` aqui: os children do BottomSheet ja vivem num `{#if open}` (BottomSheet:205),
     entao o conteudo so monta quando abre e o `carregar()` da montagem acontece na hora certa. -->
<BottomSheet {open} {onClose} ariaLabel="Configurações do servidor" wide={isDesktop} centered={isDesktop}>
  <ServerSettings {targetServer} {onOpenMotores} />
</BottomSheet>
