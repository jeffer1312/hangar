import { useCallback, useMemo, useRef, useState } from 'react';
import { useFocusEffect } from 'expo-router';
import { dreamFailure, dreamFirstMatch, getDreamConfig, getDreamStatus, type DreamConfig, type DreamStatus, type Server } from '@hangar/core';
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

  return {
    servers, dreamer, labelOf, act, busy, loading, refreshing, refresh, error, reload: load, failures,
    configs, config: dreamer ? configs[dreamer.id] ?? null : null, status: data?.status ?? null,
  };
}
