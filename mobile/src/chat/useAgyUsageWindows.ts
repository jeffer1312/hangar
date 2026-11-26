import { useEffect, useRef, useState } from 'react';
import { getCredentialsForServer } from '@hangar/core';
import { useServers } from '../stores/servers';
import { sessionCredential, type AccountSession } from './sessionAccountOptions';
import { agyUsageWindows, type UsageWindow } from './usage';

const NONE: UsageWindow[] = [];

/** Cota da conta do Antigravity, lida ao abrir a conversa, como o resumo da conta nos ajustes e o app de PC. */
export function useAgyUsageWindows(serverId: string, session: AccountSession | null): UsageWindow[] {
  const server = useServers((s) => s.servers.find((row) => row.id === serverId) ?? null);
  const agy = session?.provider === 'antigravity' ? session : null;
  const latest = useRef({ server, agy });
  latest.current = { server, agy };
  const [windows, setWindows] = useState<UsageWindow[]>(NONE);
  // A leitura depende da conta, do modelo e de qual servidor: objeto novo da lista ou da loja não relê.
  const key = server && agy ? JSON.stringify([server.id, server.baseUrl, agy.conta, agy.model]) : null;
  useEffect(() => {
    const { server, agy } = latest.current;
    if (!key || !server || !agy) { setWindows(NONE); return; }
    let active = true;
    void getCredentialsForServer(server).then((credentials) => {
      if (active) setWindows(agyUsageWindows(sessionCredential(agy, credentials)?.cota, agy.model));
    }).catch(() => { if (active) setWindows(NONE); });
    return () => { active = false; };
  }, [key]);
  return windows;
}
