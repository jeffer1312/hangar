import type { View } from 'react-native';
import { create } from 'zustand';

// Leve de propósito: as telas registram peças e ações daqui sem carregar o estado do tour (preferências, servidores).
const nodes = new Map<string, View>();

/** Quais peças estão na tela e um contador que muda quando alguma se move: o balão mede de novo. */
export const useAnchors = create<{ onScreen: string[]; moved: number }>(() => ({ onScreen: [], moved: 0 }));

export function registerAnchor(id: string, view: View) {
  nodes.set(id, view);
  const { onScreen } = useAnchors.getState();
  if (!onScreen.includes(id)) useAnchors.setState({ onScreen: [...onScreen, id].sort() });
}

export function unregisterAnchor(id: string, view: View) {
  if (nodes.get(id) !== view) return;
  nodes.delete(id);
  useAnchors.setState({ onScreen: useAnchors.getState().onScreen.filter((a) => a !== id) });
}

export function anchorMoved(id: string) {
  if (nodes.has(id)) useAnchors.setState((s) => ({ moved: s.moved + 1 }));
}

export const anchorNode = (id: string): View | null => nodes.get(id) ?? null;

// Rolagem pede nova medida no máximo a cada SCROLL_MS, com a última sempre feita (a posição em que parou).
const SCROLL_MS = 50;
let scrollTimer: ReturnType<typeof setTimeout> | null = null;
let lastScroll = 0;

/** A lista que contém peças rolou: o anel e o balão acompanham. */
export function anchorsScrolled(now: number = Date.now()) {
  if (scrollTimer || nodes.size === 0) return;
  scrollTimer = setTimeout(() => {
    scrollTimer = null;
    lastScroll = Date.now();
    useAnchors.setState((s) => ({ moved: s.moved + 1 }));
  }, Math.max(0, SCROLL_MS - (now - lastScroll)));
}

let listener: ((stepId: string) => void) | null = null;

/** O estado do tour assina aqui as ações feitas nas telas. */
export function listenTourActions(next: ((stepId: string) => void) | null) { listener = next; }

/** A pessoa fez a ação de um passo (segurou a sessão, tocou na busca…). */
export function reportTourAction(stepId: string) { listener?.(stepId); }
