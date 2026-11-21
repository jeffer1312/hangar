import { useCallback, useEffect, useRef, useState } from 'react';
import type { DownloadOptions, DownloadProgress } from './mediaCache';

// Dez quadros por segundo bastam para o anel e o "42%": o progresso nativo chega mais rápido que isso.
const FRAME_MS = 100;

function abortError(): Error {
  const e = new Error('aborted');
  e.name = 'AbortError';
  return e;
}

/**
 * Um download por vez com progresso e cancelamento. `progress` nulo = parado. Sair da tela cancela:
 * todo download daqui termina numa folha do sistema (compartilhar, "Abrir com", galeria), e ela não
 * pode abrir por cima de outra tela.
 */
export function useDownload() {
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const ctl = useRef<AbortController | null>(null);
  const alive = useRef(true);
  const last = useRef(0);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; ctl.current?.abort(); };
  }, []);

  const run = useCallback(async <T,>(job: (opts: DownloadOptions) => Promise<T>): Promise<T> => {
    // O `busy` da tela só muda no próximo render: o segundo toque rápido cai aqui e sai como cancelado.
    // Desmontado (o toque esperou um import e a tela saiu): nada mais abriria o cancelamento.
    if (ctl.current || !alive.current) throw abortError();
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
