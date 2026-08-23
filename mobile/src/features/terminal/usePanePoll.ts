import { useCallback, useEffect, useRef, useState } from 'react';
import { getPane } from '@hangar/core';
import * as m from '../../paraglide/messages';

export interface PaneSnapshot {
  text: string;
  scrollback: number;
  err: string;
  pending: boolean;
}

type GetPaneFn = (name: string, lines?: number) => Promise<{ text: string; scrollback: number }>;

// Laço do espelho — porte de TerminalMirror.svelte:75-100. Vive FORA do React (uma implementação só,
// a mesma que o hook roda e o teste exercita): setTimeout no finally (não setInterval), diff-gate,
// alive, pending quando não está no fim. `forcar` = leitura pedida pelo usuário (tecla, envio,
// carregar mais): passa por cima do congelamento, como o press()/loadMore() da barra.
export function criarPanePoller(
  getPaneFn: GetPaneFn,
  name: string,
  lines: number,
  onSnapshot: (s: PaneSnapshot) => void,
) {
  const snap: PaneSnapshot = { text: '', scrollback: 0, err: '', pending: false };
  let alive = true;
  let atBottom = true;
  let linhas = lines;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const publicar = () => onSnapshot({ ...snap });

  // Publica SÓ quando algum campo mudou: com `onSnapshot` chamado a cada tick o React re-renderiza
  // de 450 em 450ms e o diff-gate deixa de valer pra quem consome o hook.
  async function ler(forcar: boolean) {
    const p = await getPaneFn(name, linhas);
    if (!alive) return;
    let mudou = false;
    if (snap.err !== '') { snap.err = ''; mudou = true; }
    if (snap.scrollback !== p.scrollback) { snap.scrollback = p.scrollback; mudou = true; }
    const t = p.text;
    if (t === snap.text) { if (mudou) publicar(); return; }   // diff-gate
    if (!atBottom && !forcar) {
      if (!snap.pending) { snap.pending = true; mudou = true; }
      if (mudou) publicar();
      return;
    }
    snap.text = t;
    if (forcar && snap.pending) snap.pending = false;
    publicar();
  }

  function erro(e: unknown) {
    if (!alive) return;
    snap.err = e instanceof Error ? e.message : m.term_erro();
    publicar();
  }

  async function tick() {
    try {
      await ler(false);
    } catch (e) {
      erro(e);
    } finally {
      if (alive) timer = setTimeout(tick, 450);
    }
  }

  return {
    estado: () => ({ ...snap }),
    start: () => { tick(); },
    stop: () => { alive = false; clearTimeout(timer); },
    setAtBottom: (v: boolean) => {
      atBottom = v;
      if (v && snap.pending) { snap.pending = false; publicar(); }
    },
    setLines: (v: number) => { linhas = v; },
    refresh: async () => {
      try { await ler(true); } catch (e) { erro(e); }
    },
  };
}

export function usePanePoll(name: string, lines = 200) {
  const [snap, setSnap] = useState<PaneSnapshot>({ text: '', scrollback: 0, err: '', pending: false });
  const poller = useRef<ReturnType<typeof criarPanePoller> | undefined>(undefined);

  useEffect(() => {
    const p = criarPanePoller(getPane, name, lines, setSnap);
    poller.current = p;
    p.start();
    return () => { p.stop(); poller.current = undefined; };
    // `lines` não reinicia o laço de propósito (setLines abaixo) — porte do untrack da barra.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [name]);

  useEffect(() => { poller.current?.setLines(lines); }, [lines]);

  const setAtBottom = useCallback((v: boolean) => poller.current?.setAtBottom(v), []);
  const refresh = useCallback(async () => { await poller.current?.refresh(); }, []);

  return { ...snap, refresh, setAtBottom };
}
