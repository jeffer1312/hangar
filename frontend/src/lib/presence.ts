// Status "No PC / Fora" em todas as máquinas do dono: o mesmo conjunto que o enablePush assina.
import { getPresenceForServer, setPresenceForServer, type Presence, type PresenceMode } from '@hangar/core';
import type { Server } from './auth';

export interface PresenceResult {
  server: Server;
  presence?: Presence;
  error?: string;
}

// 404/405 = servidor anterior ao recurso: fica fora da conta, sem erro na tela.
function unsupported(e: unknown): boolean {
  const status = (e as { status?: number } | null)?.status;
  return status === 404 || status === 405;
}

async function eachServer(servers: Server[], call: (s: Server) => Promise<Presence>): Promise<PresenceResult[]> {
  const results = await Promise.all(servers.map(async (server): Promise<PresenceResult | null> => {
    try {
      return { server, presence: await call(server) };
    } catch (e) {
      if (unsupported(e)) return null;
      return { server, error: e instanceof Error ? e.message : String(e) };
    }
  }));
  return results.filter((r): r is PresenceResult => r !== null);
}

export function loadPresence(servers: Server[]): Promise<PresenceResult[]> {
  return eachServer(servers, getPresenceForServer);
}

export function setPresenceAll(servers: Server[], mode: PresenceMode): Promise<PresenceResult[]> {
  return eachServer(servers, (s) => setPresenceForServer(s, mode));
}

/** O que a tela mostra: o modo do servidor preferido (o desta tela), senão o do primeiro que respondeu. */
export function shownPresence(results: PresenceResult[], preferredId: string | null): Presence | null {
  const ok = results.filter((r) => r.presence);
  return (ok.find((r) => r.server.id === preferredId) ?? ok[0])?.presence ?? null;
}
