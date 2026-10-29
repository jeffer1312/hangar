import { useEffect, useMemo, useState } from 'react';
import { nextNoticeEnd, type ServerBucket } from '@hangar/core';

/** Relógio dos avisos de máquina: um timer só, até o próximo aviso vencer, redesenha a tela. */
export function useNoticeClock(byServer: readonly ServerBucket[]): number {
  // O timer pode disparar um pouco antes: `floor` garante que o aviso vencido saia mesmo assim.
  const [floor, setFloor] = useState(0);
  // Relido a cada troca das máquinas: uma que cai agora não é comparada com um instante velho.
  const now = useMemo(() => Math.max(Date.now(), floor), [byServer, floor]);
  const end = nextNoticeEnd(byServer, now);
  useEffect(() => {
    if (end === null) return;
    const timer = setTimeout(() => setFloor(end), Math.max(0, end - Date.now()));
    return () => clearTimeout(timer);
  }, [end]);
  return now;
}
