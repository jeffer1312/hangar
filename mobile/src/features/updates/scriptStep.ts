import { useEffect, useState } from 'react';

/** Próximo degrau do roteiro depois de `t`; sem degrau à frente, o fim do ciclo. */
export function nextStep(steps: readonly number[], cycle: number, t: number): number {
  return Math.min(cycle, ...steps.filter((s) => s > t));
}

/** Instante do roteiro em degraus: só re-renderiza quando o quadro muda, a transição corre na thread de UI. */
export function useScriptStep(steps: readonly number[], cycle: number, still: number, reduced: boolean): number {
  const [t, setT] = useState(reduced ? still : 0);
  useEffect(() => {
    if (reduced) { setT(still); return; }
    const started = Date.now();
    let timer: ReturnType<typeof setTimeout>;
    const tick = () => {
      const now = ((Date.now() - started) / 1000) % cycle;
      setT(now);
      timer = setTimeout(tick, (nextStep(steps, cycle, now) - now) * 1000 + 5);
    };
    tick();
    return () => clearTimeout(timer);
  }, [steps, cycle, still, reduced]);
  return t;
}
