<script lang="ts">
  // Só para o teste CreateSessionSheet.test.ts: segura `open` e alterna por clique — o $set do
  // mount é bloqueado em DEV no Svelte 5 (component_api_changed), e o cenário do bloqueador exige
  // fechar e reabrir o sheet DE VERDADE (o $effect de abertura só roda na transição).
  import CreateSessionSheet from './CreateSessionSheet.svelte';
  import type { Provider } from '../lib/types';
  import type { Server } from '../lib/auth';

  let { onCreate, onOpenSession, servidores = [], bastao = null } = $props<{
    onCreate: (name: string, cwd?: string, configDir?: string | null, provider?: Provider,
               engine?: string | null, model?: string | null, effort?: string | null) => Promise<void>;
    onOpenSession: (name: string) => void;
    /** Lista de servidores do teste (o padrão vazio mantém os casos antigos byte por byte). */
    servidores?: Server[];
    /** Origem da passagem de bastão; null = a folha normal de criar. */
    bastao?: { name: string; cwd: string; serverId: string } | null;
  }>();
  let open = $state(true);
</script>

<button type="button" data-testid="sheet-toggle" onclick={() => (open = !open)}>toggle</button>
<CreateSessionSheet {open} servers={servidores} onClose={() => (open = false)} {onCreate} {onOpenSession} {bastao} />
