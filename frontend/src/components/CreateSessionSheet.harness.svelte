<script lang="ts">
  // Só para o teste CreateSessionSheet.test.ts: segura `open` e alterna por clique — o $set do
  // mount é bloqueado em DEV no Svelte 5 (component_api_changed), e o cenário do bloqueador exige
  // fechar e reabrir o sheet DE VERDADE (o $effect de abertura só roda na transição).
  import CreateSessionSheet from './CreateSessionSheet.svelte';
  import type { Provider } from '@hangar/core';
  import type { Server } from '../lib/auth';

  let { onCreate, onOpenSession } = $props<{
    onCreate: (name: string, cwd?: string, configDir?: string | null, provider?: Provider,
               engine?: string | null, model?: string | null, effort?: string | null) => Promise<void>;
    onOpenSession: (name: string) => void;
  }>();
  let open = $state(true);
  let servers = $state<Server[]>([]);
</script>

<button type="button" data-testid="sheet-toggle" onclick={() => (open = !open)}>toggle</button>
<CreateSessionSheet {open} {servers} onClose={() => (open = false)} {onCreate} {onOpenSession} />
