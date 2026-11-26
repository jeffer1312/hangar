import { useEffect, useState } from 'react';

/** Relógio de um segundo enquanto `running`; parado, fica a última hora lida. Não lê o servidor. */
export function useNow(running = true): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);
  return now;
}
