import { useCallback, useEffect, useRef, useState } from 'react';
import type { DownloadOptions, DownloadProgress } from './mediaCache';

// Dez quadros por segundo bastam para o anel e o "42%": o progresso nativo chega mais rápido que isso.
const FRAME_MS = 100;

/**
 * Um download por vez com progresso e cancelamento. `progress` nulo = parado. O download segue se o
 * componente sair da tela (a lista desmonta a linha ao rolar); só a tela para de ser atualizada.
 */
export function useDownload() {
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const ctl = useRef<AbortController | null>(null);
  const alive = useRef(true);
  const last = useRef(0);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const run = useCallback(async <T,>(job: (opts: DownloadOptions) => Promise<T>): Promise<T> => {
    // O `busy` da tela só muda no próximo render: o segundo toque rápido cai aqui e sai como cancelado.
    if (ctl.current) {
      const e = new Error('The operation was aborted.');
      e.name = 'AbortError';
      throw e;
    }
    const c = new AbortController();
    ctl.current = c;
    last.current = 0;
    setProgress({ written: 0, total: null });
    const onProgress = (p: DownloadProgress) => {
      const now = Date.now();
      if (!alive.current || ctl.current !== c || now - last.current < FRAME_MS) return;
      last.current = now;
      setProgress(p);
    };
    try {
      return await job({ onProgress, signal: c.signal });
    } finally {
      if (ctl.current === c) {
        ctl.current = null;
        if (alive.current) setProgress(null);
      }
    }
  }, []);

  const cancel = useCallback(() => ctl.current?.abort(), []);

  return { progress, busy: progress !== null, run, cancel };
}

export const percentOf = (p: DownloadProgress | null): number | null =>
  p && p.total ? Math.min(100, Math.floor((p.written * 100) / p.total)) : null;
