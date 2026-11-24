import { useCallback, useEffect, useRef, useState } from 'react';
import { getCodexModels, getPermissionModes, setCodexMode, setPermissionMode } from '@hangar/core';
import { chatStore } from '../../stores/chat';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';

// Ordem do segmentado, do mais cauteloso ao mais solto.
const CLAUDE_MODES = ['manual', 'auto', 'acceptEdits', 'plan', 'bypassPermissions', 'dontAsk'];
const CODEX_MODES = ['default', 'plan'];
// O ciclo do shift+tab do agy, com os nomes do Claude; `bypassPermissions` só existe na abertura.
const ANTIGRAVITY_MODES = ['manual', 'auto', 'acceptEdits', 'plan'];

interface Args {
  serverId: string;
  name: string;
  // null: provider sem modo de permissão (Pi, Kimi, omp, motores) — nada é lido.
  provider: 'claude' | 'codex' | 'antigravity' | null;
}

// Modo da sessão: no Claude é o modo de permissão (ciclo lido da própria sessão), no Codex é
// Normal/Planejar. O Antigravity usa o campo e as rotas do Claude, com ciclo fixo. O valor segue o SSE.
export function usePermissionControl({ serverId, name, provider }: Args) {
  const chat = chatStore(serverId, name);
  // Servidor da rota, não o ativo: o ativo pode ser outra máquina sem esta sessão. Sem ele (lista
  // ainda carregando ou máquina removida) nada é pedido: `null` no core cairia no ativo.
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  const isCodex = provider === 'codex';
  const isClaude = provider === 'claude';
  const isAntigravity = provider === 'antigravity';
  const claudeRoutes = isClaude || isAntigravity;
  const sseClaude = chat.use((s) => s.stateEvent?.claude_permission_mode ?? null);
  const sseCodex = chat.use((s) => s.stateEvent?.codex_mode ?? null);
  const sessionState = chat.use((s) => s.stateEvent?.state ?? null);

  const [current, setCurrent] = useState<string | null>(null);
  const [modes, setModes] = useState<string[]>([]);
  const [probeable, setProbeable] = useState(true);
  const [probing, setProbing] = useState(false);
  const [applying, setApplying] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  // Toda leitura leva um número: resposta velha (ou anterior a uma troca pelo SSE) não pisa na nova.
  const seq = useRef(0);
  // O backend disse "só vale para Claude" (sessão Pi/Kimi que chegou como claude): não insistir.
  const refused = useRef(false);

  useEffect(() => {
    if (!claudeRoutes || !sseClaude) return;
    seq.current++;
    setCurrent(sseClaude);
  }, [claudeRoutes, sseClaude]);

  useEffect(() => {
    if (isCodex && sseCodex) setCurrent(sseCodex);
  }, [isCodex, sseCodex]);

  // Claude: lê o modo atual sem teclas a cada mudança de estado; o ciclo só quando pedido.
  useEffect(() => {
    if (!claudeRoutes || refused.current || !server) return;
    const my = ++seq.current;
    getPermissionModes(name, false, server)
      .then((res) => {
        if (my !== seq.current) return;
        setCurrent(res.current);
        if (res.modes.length) setModes(res.modes);
        setProbeable(res.sondavel);
      })
      .catch((e: unknown) => {
        if ((e as { code?: string })?.code === 'erro_permissao_so_claude') refused.current = true;
      });
  }, [claudeRoutes, name, server, sessionState]);

  useEffect(() => {
    if (!isCodex || !server) return;
    let alive = true;
    getCodexModels(name, server)
      .then((res) => { if (alive && res.current.mode) setCurrent(res.current.mode); })
      .catch(() => { /* sem catálogo o chip fica sem o ícone do modo */ });
    return () => { alive = false; };
  }, [isCodex, name, server]);

  // A sonda percorre o ciclo com Shift+Tab e volta; sem ela o servidor devolve [] até ter cache.
  const probe = useCallback(async () => {
    if (!isClaude || !probeable || modes.length > 0 || !server) return;
    const my = ++seq.current;
    setProbing(true);
    setNotice(null);
    try {
      const res = await getPermissionModes(name, true, server);
      if (my !== seq.current) return;
      setCurrent(res.current);
      setModes(res.modes);
      setProbeable(res.sondavel);
      if (res.restaurado === false) setNotice(m.native_mode_probe_not_restored());
    } catch (e) {
      setNotice(e instanceof Error ? e.message : String(e));
    } finally {
      setProbing(false);
    }
  }, [isClaude, probeable, modes.length, name, server]);

  // Diz se a troca pegou: o painel só fecha no sucesso, para o aviso de falha ficar à vista.
  const select = useCallback(async (mode: string): Promise<boolean> => {
    if (applying || mode === current) return false;
    if (!server) { setNotice(m.chat_servidor_removido()); return false; }
    setApplying(true);
    setNotice(null);
    try {
      let ficou: string | null | undefined;
      if (isCodex) {
        ficou = (await setCodexMode(name, mode as 'default' | 'plan', server)).mode;
      } else {
        seq.current++;
        const res = await setPermissionMode(name, mode, server);
        // O backend devolve o que FICOU, que pode não ser o pedido.
        ficou = res.mode ?? res.current;
      }
      // Resposta sem o modo: mostrar o pedido como aplicado seria um sucesso inventado.
      if (!ficou) {
        setNotice(m.native_mode_not_confirmed());
        return false;
      }
      setCurrent(ficou);
      return true;
    } catch (e) {
      setNotice(e instanceof Error ? e.message : String(e));
      if (!isCodex) {
        // Mostrar o modo antigo depois de uma troca que pode ter pegado afirma uma permissão falsa.
        getPermissionModes(name, false, server).then((res) => {
          setCurrent(res.current);
          if (res.modes.length) setModes(res.modes);
        }).catch(() => {});
      }
      return false;
    } finally {
      setApplying(false);
    }
  }, [applying, current, isCodex, name, server]);

  // Só o que o ciclo da sessão alcança; modo novo do CLI entra cru no fim. Sem ciclo lido ainda,
  // o atual fica à vista sozinho.
  const options = isCodex ? CODEX_MODES
    : isAntigravity ? CLAUDE_MODES.filter((mo) => ANTIGRAVITY_MODES.includes(mo) || mo === current)
    : modes.length
      ? [...CLAUDE_MODES.filter((mo) => modes.includes(mo) || mo === current), ...modes.filter((mo) => !CLAUDE_MODES.includes(mo))]
      : current ? [current] : [];
  const cycleUnknown = isClaude && modes.length === 0;

  return {
    enabled: isClaude || isCodex || isAntigravity,
    current,
    options,
    // Ciclo ainda não lido: o botão "Ler do terminal" aparece; sem sonda possível, o aviso de dontAsk.
    canProbe: cycleUnknown && probeable,
    noCycle: cycleUnknown && !probeable,
    probing,
    applying,
    notice,
    probe,
    select,
  };
}
