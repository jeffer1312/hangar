/**
 * Ponte pro navegador embutido do shell Electron (shell/preload.cjs → IPC 'hangar:nav-*' →
 * WebContentsView, em shell/main.cjs). UM view por sessão: a chave é o workspaceSessionKey
 * (serverId::nome). Trocar de sessão chama `hide` (o view fica vivo e o agente segue dirigindo
 * via CDP); fechar de verdade é só o × (close). Fora do shell (navegador, celular, PWA) a ponte
 * não existe e o NavegadorPane cai no iframe de sempre.
 *
 * Lida a cada chamada, nunca em const de módulo — o preload injeta antes da página, mas teste
 * monta componente depois (mesmo motivo do pastaNativa).
 */
export type NavBounds = { x: number; y: number; width: number; height: number };

export type NavNativo = {
  /** Cria ou reexibe o view da sessão. Sem `url`: só reexibe — ok:false se o main não tem o view. */
  open: (chave: string, url: string | undefined, bounds: NavBounds) => Promise<{ ok: boolean }>;
  hide: (chave: string) => void;
  bounds: (chave: string, b: NavBounds) => void;
  reload: (chave: string) => void;
  close: (chave: string) => void;
  /** Cookies do Chrome real (CDP) pro view. Opcional: shell antigo não tem. Nunca rejeita. */
  importCookies?: (chave: string, host: string, porta?: number) =>
    Promise<{ ok: boolean; gravados: number; falhos: number; erro?: string; detalhe?: string }>;
};

export function navegadorNativo(): NavNativo | undefined {
  return (window as { hangar?: { nav?: NavNativo } }).hangar?.nav;
}
