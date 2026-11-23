import { openConnectPreview, startPreview } from './api';
import type { Server } from './servers';

export type PortPreviewVia = 'connect' | 'tailscale';

/** A porta respondeu, mas não como página (Postgres, Redis): o servidor recusa com `port_not_http`. */
export class PortNotPageError extends Error {
  constructor(readonly port: number) {
    super(`port ${port} is not a page`);
  }
}

// Respostas em que o Connect não serve nesta máquina e o Tailscale ainda pode servir.
// `not_linked` e 404 (servidor sem a rota) dizem que não há Connect; os outros, que há e está fora.
const NO_CONNECT = new Set(['connect_not_linked']);
const CONNECT_DOWN = new Set(['connect_not_running', 'connect_preview_port_closed']);

type Deps = {
  connect: (server: Server, port: number) => Promise<{ url: string }>;
  tailscale: (port: number, server: Server) => Promise<{ url: string }>;
};

const defaults: Deps = { connect: openConnectPreview, tailscale: startPreview };

const notPage = (e: unknown) => (e as { code?: string }).code === 'port_not_http';

/** Abre a prévia de uma porta da sessão: pelo Connect quando a máquina o tem ligado, senão pelo
 * Tailscale. As duas são uma por vez: abrir outra porta fecha a anterior. */
export async function openPortPreview(server: Server, port: number, deps: Deps = defaults): Promise<{ url: string; via: PortPreviewVia }> {
  let connectError: unknown;
  try {
    return { url: (await deps.connect(server, port)).url, via: 'connect' };
  } catch (e) {
    if (notPage(e)) throw new PortNotPageError(port);
    const { status, code } = e as { status?: number; code?: string };
    const absent = status === 404 || (code !== undefined && NO_CONNECT.has(code));
    if (!absent && !(code !== undefined && CONNECT_DOWN.has(code))) throw e;
    if (!absent) connectError = e;
  }
  try {
    return { url: (await deps.tailscale(port, server)).url, via: 'tailscale' };
  } catch (e) {
    if (notPage(e)) throw new PortNotPageError(port);
    // Máquina com Connect ligado: o erro que explica é o dele, não o do Tailscale de reserva.
    throw connectError ?? e;
  }
}
