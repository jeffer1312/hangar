import { useEffect, useRef, useState } from 'react';
import { getCredentialsForServer, registrarDiag } from '@hangar/core';
import { useServers } from '../stores/servers';
import { sessionCredential, type AccountSession } from './sessionAccountOptions';
import { agyUsageWindows, type UsageWindow } from './usage';

const NONE: UsageWindow[] = [];

/** Cota da conta do Antigravity, lida ao abrir a conversa e a cada fim de turno, sem leitura periódica. */
export function useAgyUsageWindows(serverId: string, session: AccountSession | null, idle: boolean): UsageWindow[] {
  const server = useServers((s) => s.servers.find((row) => row.id === serverId) ?? null);
  const agy = session?.provider === 'antigravity' ? session : null;
  const latest = useRef({ server, agy });
  latest.current = { server, agy };
  const [windows, setWindows] = useState<UsageWindow[]>(NONE);
  const [round, setRound] = useState(0);
  useEffect(() => { if (idle) setRound((n) => n + 1); }, [idle]);
  // A leitura depende da conta, do modelo, do servidor e do turno: objeto novo da lista ou da loja não relê.
  const account = server && agy ? JSON.stringify([server.id, server.baseUrl, agy.conta, agy.model]) : null;
  useEffect(() => { setWindows(NONE); }, [account]);
  useEffect(() => {
    const { server, agy } = latest.current;
    if (!account || !server || !agy) return;
    let active = true;
    void getCredentialsForServer(server).then((credentials) => {
      if (active) setWindows(agyUsageWindows(sessionCredential(agy, credentials)?.cota, agy.model));
    }).catch((err: unknown) => {
      // Sem a leitura o rodapé só deixa de mostrar a cota; a causa fica no diário.
      registrarDiag({ evento: 'cota.agy_falhou', nivel: 'aviso', tela: 'chat', provider: 'antigravity',
        detalhe: err instanceof Error ? err.message : String(err) }, server.baseUrl);
    });
    return () => { active = false; };
  }, [account, round]);
  return windows;
}
