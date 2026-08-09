<script lang="ts">
  // Harness de teste (PushQuiet.test.ts): expõe o estado via container $state exportado — o proxy
  // devolvido pelo `mount()` do Svelte 5 permite LER/ESCREVER fields de $state exportado (desde que
  // o $state não seja reatribuído, só mutado). O teste troca alvo/abertura como um pai real faria
  // (objeto NOVO a cada troca, igual recomputo). Não é usado no app.
  import PushQuiet from './PushQuiet.svelte';
  import type { PushTarget } from '../lib/quietHours';

  export const est = $state<{ alvo: PushTarget; aberto: boolean }>({ alvo: { mode: 'global' }, aberto: true });

  export function setarAlvo(novo: PushTarget): void { est.alvo = novo; }
  export function setarAberto(v: boolean): void { est.aberto = v; }
</script>

<PushQuiet target={est.alvo} open={est.aberto} />
