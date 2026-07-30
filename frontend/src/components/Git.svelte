<script lang="ts">
  // Dono do modal de git: a folha, o store da sessao e nada mais. O corpo inteiro (cabecalho, abas,
  // faixa) e do GitTabs — este arquivo substitui o GitSheet (celular) E o GitPanel (desktop), que
  // eram duas implementacoes da mesma tela.
  import BottomSheet from './BottomSheet.svelte';
  import GitTabs from './git/GitTabs.svelte';
  import { createGitStore } from '../lib/gitStore.svelte';

  // `desktop` por PROP, nao matchMedia proprio: o GitSheet era a terceira copia da mesma media
  // query (App.svelte, BottomSheet.svelte) e a primeira pintura saia mobile.
  interface Props { open: boolean; sessionName: string; desktop: boolean; onClose: () => void }
  let { open, sessionName, desktop, onClose }: Props = $props();

  // Dono do store — era do GitSheet, COM o guard que evita recriar a cada render. Sem ele, trocar de
  // sessao com o modal aberto mostraria o git da anterior.
  // svelte-ignore state_referenced_locally
  // Ler `sessionName` aqui pega so o valor inicial de proposito: quem acompanha a troca e o efeito
  // logo abaixo, que recria o store — um $derived nao serve, o store guarda estado vivo.
  let git = $state(createGitStore(sessionName));
  $effect(() => { if (git.sessionName !== sessionName) git = createGitStore(sessionName); });
  $effect(() => { if (open) git.load(); });
</script>

<!-- `wide` + `centered` = no desktop a folha JA vira modal centrado min(1100px, 92vw). Mesmo par que
     o EnginesSheet usa. Nao usar ModalDialog: a folha fica em z 100, e o 110/120 do CommitMenu
     segue correto. -->
<BottomSheet {open} {onClose} ariaLabel="Git" wide={desktop} centered={desktop}>
  <GitTabs {git} {desktop} {onClose} />
</BottomSheet>
