import { useSyncExternalStore } from 'react';
import { prefs } from '../../stores/prefs';

// Aberto/fechado da seção "Ocultas" é do aparelho; nasce fechada. Assinável porque a página
// Sessões abre a seção com a lista já montada por baixo.
const KEY = 'cp_hidden_section_open';
const listeners = new Set<() => void>();
let open: boolean | null = null;

function read(): boolean {
  if (open === null) {
    try { open = prefs.getBoolean(KEY) ?? false; } catch { open = false; }
  }
  return open;
}

export function setHiddenSectionOpen(value: boolean) {
  open = value;
  try { prefs.set(KEY, value); } catch { /* sem armazenamento: vale até fechar o app */ }
  for (const l of listeners) l();
}

export function useHiddenSectionOpen(): boolean {
  return useSyncExternalStore((l) => { listeners.add(l); return () => { listeners.delete(l); }; }, read);
}
