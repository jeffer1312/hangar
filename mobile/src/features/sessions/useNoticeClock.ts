import { useEffect, useState } from 'react';
import { nextNoticeEnd, type ServerBucket } from '@hangar/core';

/** Relógio dos avisos de máquina: um timer só, até o próximo aviso vencer, redesenha a tela. */
export function useNoticeClock(byServer: readonly ServerBucket[]): number {
  const [now, setNow] = useState(Date.now);
  const end = nextNoticeEnd(byServer, now);
  useEffect(() => {
    if (end === null) return;
    const timer = setTimeout(() => setNow(Date.now()), Math.max(0, end - Date.now()));
    return () => clearTimeout(timer);
  }, [end]);
  return now;
}
