import { useEffect, useState } from 'react';
import { nextNoticeEnd, type ServerBucket } from '@hangar/core';

/** Relógio dos avisos de máquina: um timer só, até o próximo aviso vencer, redesenha a tela. */
export function useNoticeClock(byServer: readonly ServerBucket[]): number {
  const [now, setNow] = useState(Date.now);
  const end = nextNoticeEnd(byServer, now);
  useEffect(() => {
    if (end === null) return;
    // O timer pode disparar um pouco antes: sem o `max`, o aviso ficaria preso até outro evento.
    const timer = setTimeout(() => setNow(Math.max(Date.now(), end)), Math.max(0, end - Date.now()));
    return () => clearTimeout(timer);
  }, [end]);
  return now;
}
