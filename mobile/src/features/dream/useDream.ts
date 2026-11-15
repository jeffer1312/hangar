import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useFocusEffect } from 'expo-router';
import { dreamFailure, dreamFirstMatch, getDreamConfig, getDreamStatus, openDreamEvents, stopDream, type DreamConfig, type DreamProgress,
  type DreamStatus, type Server } from '@hangar/core';
import { ownServers, useServers } from '../../stores/servers';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

type Data = { dreamerId: string; status: DreamStatus };

/**
 * Configuração, estado e ações do sonho valem na máquina que sonha: o servidor ativo só diz qual é.
 * Relê ao ganhar foco, porque a tela de projeto e a de rodada mudam o estado por baixo.
 */
export function useDream() {
  const all = useServers((s) => s.servers);
  const servers = useMemo(() => ownServers(all), [all]);
  const active = useServers((s) => s.active());
  const [data, setData] = useState<Data | null>(null);
  const [configs, setConfigs] = useState<Record<string, DreamConfig>>({});
  // Motivo de cada máquina cuja configuração não veio: não é o mesmo que "sem sonho configurado".
  const [failures, setFailures] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const operation = useRef(false);
  const loaded = useRef(false);

  const load = useCallback(async () => {
    const g = ++generation.current;
    const live = () => g === generation.current;
    if (!active || active.invite) { setLoading(false); setError(m.dream_load_error()); return; }
    // Releitura com a tela já montada é calada: nada de "carregando" nem de girar o puxar.
    if (!loaded.current) setLoading(true);
    setError('');
    // As outras máquinas entram conforme respondem; uma lenta ou fora do ar não segura a tela.
    const reads = servers.filter((s) => s.id !== active.id).map((s) => getDreamConfig(s).then((config) => {
      if (live()) {
        setConfigs((c) => ({ ...c, [s.id]: config }));
        setFailures((f) => { const next = { ...f }; delete next[s.id]; return next; });
      }
      return { server: s, config };
    }, (e: unknown) => {
      if (live()) setFailures((f) => ({ ...f, [s.id]: dreamFailure(e) }));
      return null;
    }));
    try {
      const own = await getDreamConfig(active);
      if (!live()) return;
      setConfigs((c) => ({ ...c, [active.id]: own }));
      // Máquina escolhida que o app não conhece (ou ainda nenhuma): o ativo responde por ela.
      const dreamer = !own.machine || own.machine === own.this_machine ? active
        : (await dreamFirstMatch(reads, (r) => r?.config.this_machine === own.machine))?.server ?? active;
      const status = await getDreamStatus(dreamer);
      if (live()) { loaded.current = true; setData({ dreamerId: dreamer.id, status }); }
    } catch (e) {
      if (live()) setError(`${m.dream_load_error()} · ${dreamFailure(e)}`);
    } finally {
      if (live()) setLoading(false);
    }
  }, [active, servers]);

  useFocusEffect(useCallback(() => { void load(); return () => { generation.current++; }; }, [load]));

  /** Só o gesto de puxar gira o indicador. */
  const refresh = useCallback(async () => {
    setRefreshing(true);
    try { await load(); } finally { setRefreshing(false); }
  }, [load]);

  const dreamer: Server | null = data ? servers.find((s) => s.id === data.dreamerId) ?? null : null;

  // Andamento ao vivo da máquina que sonha, só com a tela em foco. `finished` guarda o retrato da rodada
  // que acabou enquanto a tela estava aberta: é ele que mostra "Sonhou há…" ou "O sonho parou".
  const [progress, setProgress] = useState<DreamProgress | null>(null);
  const [finished, setFinished] = useState<DreamProgress | null>(null);
  const latest = useRef<DreamProgress | null>(null);
  // O evento ao vivo é mais novo que o status: depois do primeiro, o status só traz rodada que o evento não conhece,
  // e nunca uma que o evento já deu por acabada (status lido antes do fim a ressuscitaria).
  const sawEvent = useRef(false);
  const ended = useRef<string | null>(null);
  const statusProgress = data?.status.progress ?? null;
  useEffect(() => {
    if (statusProgress?.run_id && statusProgress.run_id === ended.current) return;
    if (sawEvent.current && (!statusProgress || statusProgress.run_id === latest.current?.run_id)) return;
    latest.current = statusProgress;
    setProgress(statusProgress);
  }, [statusProgress]);
  const dreamerRef = useRef(dreamer);
  dreamerRef.current = dreamer;
  const loadRef = useRef(load);
  loadRef.current = load;
  useFocusEffect(useCallback(() => {
    const server = dreamerRef.current;
    if (!server) return;
    sawEvent.current = false;
    const source = openDreamEvents(server, (next) => {
      sawEvent.current = true;
      const before = latest.current;
      latest.current = next;
      setProgress(next);
      if (next) { if (before?.run_id !== next.run_id) setFinished(null); return; }
      // A rodada acabou: o retrato final fica na tela e a lista relê os últimos sonhos.
      if (before) { ended.current = before.run_id; setFinished(before); void loadRef.current(); }
    });
    return () => source.close();
  }, [data?.dreamerId]));
  const labelOf = useCallback((machine: string) =>
    servers.find((s) => configs[s.id]?.this_machine === machine)?.label ?? machine, [servers, configs]);

  /**
   * Ação da pessoa: uma por vez, toast com vibração no resultado e releitura antes de responder, para quem espera
   * já ver o valor do servidor. Texto devolvido = falha parcial. `false` = falhou ou foi recusada.
   */
  const act = useCallback(async (action: () => Promise<unknown>, success?: string): Promise<boolean> => {
    if (operation.current) return false;
    operation.current = true; setBusy(true);
    try {
      const failed = await action();
      if (typeof failed === 'string') { toast.erro(failed); return false; }
      if (success) toast.ok(success); else haptic.success();
      return true;
    } catch (e) {
      toast.erro(dreamFailure(e));
      return false;
    } finally {
      operation.current = false; setBusy(false);
      await load();
    }
  }, [load]);

  const stop = useCallback(() => {
    const server = dreamerRef.current;
    if (server) void act(() => stopDream(server));
  }, [act]);

  return {
    servers, dreamer, labelOf, act, busy, loading, refreshing, refresh, error, reload: load, failures,
    configs, config: dreamer ? configs[dreamer.id] ?? null : null, status: data?.status ?? null,
    progress, finished, stop,
  };
}
