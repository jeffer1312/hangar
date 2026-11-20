import { useCallback, useEffect, useRef, useState } from 'react';
import { getNewChatCommandsForServer, type CommandInfo, type Server } from '@hangar/core';

// A origem da lista na tela sem sessão: a conta e a pasta que a conversa vai usar (só Claude).
export type NewChatCommandSource = { cwd: string | null; configDir: string | null; headless: boolean };

const cache = new Map<string, CommandInfo[]>();
// Pedido mais recente por chave, entre montagens: resposta atrasada não sobrescreve a mais nova.
const latestRequest = new Map<string, number>();
let requestSeq = 0;

// O mesmo guardar e revalidar de useSessionCommands, para a conversa que ainda vai nascer. Cada
// abertura do `/` relê: a primeira resposta é a lista do disco, e a seguinte já traz a da CLI.
export function useNewChatCommands(server: Server, source: NewChatCommandSource | null, wanted: boolean) {
  const key = source ? `${server.id}|new|${source.configDir ?? ''}|${source.cwd ?? ''}|${source.headless}` : '';
  const [loaded, setLoaded] = useState<{ key: string; commands: CommandInfo[] | null }>(() => ({ key, commands: cache.get(key) ?? null }));
  const commands = !source ? null : loaded.key === key ? loaded.commands : cache.get(key) ?? null;
  const [error, setError] = useState('');
  const [attempt, setAttempt] = useState(0);
  const shown = useRef(false);
  shown.current = commands !== null;
  // Lidos na hora do pedido: o objeto do servidor pode mudar de identidade a cada desenho da tela.
  const latest = useRef({ server, source });
  latest.current = { server, source };

  useEffect(() => {
    const { server: target, source: chosen } = latest.current;
    if (!wanted || !chosen) return;
    let alive = true;
    // Revalidação que falha não troca a lista que está na tela pelo erro.
    const revalidating = shown.current;
    setError('');
    const seq = ++requestSeq;
    latestRequest.set(key, seq);
    getNewChatCommandsForServer(target, chosen.cwd, chosen.configDir, chosen.headless)
      .then((c) => {
        if (latestRequest.get(key) !== seq) return;
        cache.set(key, c);
        if (alive) setLoaded({ key, commands: c });
      })
      .catch((e: unknown) => {
        if (latestRequest.get(key) !== seq || !alive || revalidating) return;
        // Servidor antigo não tem a rota: a tela nova fica sem lista, como antes, e não em erro.
        if ((e as { status?: unknown } | null)?.status === 404) setLoaded({ key, commands: [] });
        else setError(e instanceof Error ? e.message : String(e));
      });
    return () => { alive = false; };
  }, [wanted, key, attempt]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  return { commands, error, retry };
}
