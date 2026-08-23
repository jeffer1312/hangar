import { useCallback, useEffect, useRef, useState } from 'react';
import { getPane } from '@hangar/core';
import * as m from '../../paraglide/messages';

// Hook adaptativo — porte de TerminalMirror.svelte:75-100
// Laço com setTimeout no finally (não setInterval), diff-gate, alive, pending quando não está no fim.
export function usePanePoll(name: string, lines = 200) {
  const [text, setText] = useState('');
  const [scrollback, setScrollback] = useState(0);
  const [err, setErr] = useState('');
  const [pending, setPending] = useState(false);

  const textRef = useRef(text);
  textRef.current = text;
  const linesRef = useRef(lines);
  linesRef.current = lines;
  const atBottomRef = useRef(true);
  const pendingRef = useRef(pending);
  pendingRef.current = pending;
  const aliveRef = useRef(true);
  const timerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const setAtBottom = useCallback((v: boolean) => {
    atBottomRef.current = v;
    if (v) setPending(false);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const p = await getPane(name, linesRef.current);
      if (!aliveRef.current) return;
      setErr('');
      setScrollback(p.scrollback);
      const t = p.text;
      if (t === textRef.current) return;
      if (!atBottomRef.current) {
        setPending(true);
        return;
      }
      setText(t);
    } catch (e) {
      if (aliveRef.current) setErr(e instanceof Error ? e.message : m.term_erro());
    }
  }, [name]);

  useEffect(() => {
    aliveRef.current = true;
    async function tick() {
      try {
        const p = await getPane(name, linesRef.current);
        if (!aliveRef.current) return;
        setErr('');
        setScrollback(p.scrollback);
        const t = p.text;
        if (t === textRef.current) return; // diff-gate
        if (!atBottomRef.current) {
          setPending(true);
          return; // congelado: não puxa tapete
        }
        setText(t);
      } catch (e) {
        if (aliveRef.current) setErr(e instanceof Error ? e.message : m.term_erro());
      } finally {
        if (aliveRef.current) timerRef.current = setTimeout(tick, 450);
      }
    }
    tick();
    return () => {
      aliveRef.current = false;
      clearTimeout(timerRef.current);
    };
  }, [name]);

  useEffect(() => {
    linesRef.current = lines;
  }, [lines]);

  return { text, scrollback, err, pending, refresh, setAtBottom, atBottom: atBottomRef.current };
}

// Helper testável sem React — usado pelo teste unitário para mockar getPane.
// Mantém a mesma semântica do hook (diff-gate, alive, erro continua) para mutação detectar.
export function _createPanePollerForTest(
  getPaneFn: (name: string, lines?: number) => Promise<{ text: string; scrollback: number }>,
  name: string,
  lines = 200,
) {
  let text = '';
  let scrollback = 0;
  let err = '';
  let pending = false;
  let alive = true;
  let atBottom = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let setCount = 0;

  const getState = () => ({ text, scrollback, err, pending, setCount, alive, atBottom });

  async function tick() {
    try {
      const p = await getPaneFn(name, lines);
      if (!alive) return;
      err = '';
      scrollback = p.scrollback;
      const t = p.text;
      if (t === text) return;
      if (!atBottom) {
        pending = true;
        return;
      }
      text = t;
      setCount++;
    } catch (e) {
      if (alive) err = e instanceof Error ? e.message : 'erro';
    } finally {
      if (alive) timer = setTimeout(tick, 450);
    }
  }

  function start() {
    tick();
  }
  function stop() {
    alive = false;
    clearTimeout(timer);
  }
  function setAtBottom(v: boolean) {
    atBottom = v;
    if (v) pending = false;
  }
  function setLines(v: number) {
    lines = v;
  }
  async function refresh() {
    try {
      const p = await getPaneFn(name, lines);
      if (!alive) return;
      err = '';
      scrollback = p.scrollback;
      const t = p.text;
      if (t === text) return;
      if (!atBottom) {
        pending = true;
        return;
      }
      text = t;
      setCount++;
    } catch (e) {
      if (alive) err = e instanceof Error ? e.message : 'erro';
    }
  }

  return { getState, start, stop, setAtBottom, setLines, refresh, _tick: tick };
}
