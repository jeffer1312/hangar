import { useCallback, useEffect, useRef, useState } from 'react';
import { AppState } from 'react-native';

/** A leitura anterior nunca substitui outra máquina/revisão; segundo plano não mantém consultas. */
export function useReviewResource<T>(key: string, load: (signal: AbortSignal, publish: (data: T) => void) => Promise<T>, active: boolean) {
  const scope = useRef({ key, active });
  if (scope.current.key !== key || scope.current.active !== active) scope.current = { key, active };
  const loader = useRef(load); loader.current = load;
  const seq = useRef(0);
  const request = useRef<AbortController | null>(null);
  const pending = useRef(false);
  const [state, setState] = useState<{ key: string; data?: T; loading: boolean; error: string }>({ key, loading: true, error: '' });
  const refresh = useCallback(async () => {
    const expected = scope.current;
    if (!expected.active || AppState.currentState !== 'active') return undefined;
    request.current?.abort();
    const controller = new AbortController(); request.current = controller;
    const attempt = ++seq.current;
    const current = () => scope.current === expected && seq.current === attempt && !controller.signal.aborted;
    pending.current = true;
    setState((previous) => ({ key: expected.key, data: previous.key === expected.key ? previous.data : undefined, loading: true, error: '' }));
    try {
      const data = await loader.current(controller.signal, (partial) => {
        if (current()) setState({ key: expected.key, data: partial, loading: true, error: '' });
      });
      if (current()) setState({ key: expected.key, data, loading: false, error: '' });
      return current() ? data : undefined;
    } catch (error) {
      if (current()) setState((previous) => ({ ...previous, loading: false, error: error instanceof Error ? error.message : String(error) }));
      return undefined;
    } finally {
      if (seq.current === attempt) pending.current = false;
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    if (AppState.currentState === 'active') void refresh();
    const timer = setInterval(() => { if (AppState.currentState === 'active' && !pending.current) void refresh(); }, 15_000);
    const subscription = AppState.addEventListener('change', (next) => {
      if (next === 'active') void refresh();
      else { seq.current++; request.current?.abort(); pending.current = false; }
    });
    return () => { seq.current++; clearInterval(timer); subscription.remove(); request.current?.abort(); pending.current = false; };
  }, [key, active, refresh]);

  return { data: state.key === key ? state.data : undefined, loading: state.key !== key || state.loading,
    error: state.key === key ? state.error : '', refresh };
}
