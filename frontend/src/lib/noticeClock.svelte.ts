import { nextNoticeEnd, type ServerBucket } from '@hangar/core';

/** Relógio dos avisos de máquina: um timer só, até o próximo aviso vencer, redesenha a tela.
 *  Chamar na inicialização do componente (usa `$effect`). */
export function noticeClock(buckets: () => readonly ServerBucket[]): { readonly now: number } {
  let now = $state(Date.now());
  const end = $derived(nextNoticeEnd(buckets(), now));
  $effect(() => {
    if (end === null) return;
    const timer = setTimeout(() => { now = Date.now(); }, Math.max(0, end - Date.now()));
    return () => clearTimeout(timer);
  });
  return { get now() { return now; } };
}
