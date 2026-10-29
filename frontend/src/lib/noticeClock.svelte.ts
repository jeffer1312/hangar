import { nextNoticeEnd, type ServerBucket } from '@hangar/core';

/** Relógio dos avisos de máquina: um timer só, até o próximo aviso vencer, redesenha a tela.
 *  Chamar na inicialização do componente (usa `$effect`). */
export function noticeClock(buckets: () => readonly ServerBucket[]): { readonly now: number } {
  // O timer pode disparar um pouco antes: `floor` garante que o aviso vencido saia mesmo assim.
  let floor = $state(0);
  // Relido a cada troca das máquinas: uma que cai agora não é comparada com um instante velho.
  const now = $derived.by(() => { buckets(); return Math.max(Date.now(), floor); });
  const end = $derived(nextNoticeEnd(buckets(), now));
  $effect(() => {
    if (end === null) return;
    const timer = setTimeout(() => { floor = end; }, Math.max(0, end - Date.now()));
    return () => clearTimeout(timer);
  });
  return { get now() { return now; } };
}
