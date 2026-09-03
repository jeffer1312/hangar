/**
 * Ponte pro navegador embutido do shell Electron (shell/preload.cjs → IPC 'hangar:nav-*' →
 * WebContentsView, em shell/main.cjs). Fora do shell (navegador, celular, PWA) a ponte não existe
 * e o NavegadorPane cai no iframe de sempre.
 *
 * Lida a cada chamada, nunca em const de módulo — o preload injeta antes da página, mas teste
 * monta componente depois (mesmo motivo do pastaNativa).
 */
export type NavBounds = { x: number; y: number; width: number; height: number };

export type NavNativo = {
  open: (url: string, bounds: NavBounds) => Promise<{ ok: boolean }>;
  bounds: (b: NavBounds) => void;
  reload: () => void;
  close: () => void;
};

export function navegadorNativo(): NavNativo | undefined {
  return (window as { hangar?: { nav?: NavNativo } }).hangar?.nav;
}
