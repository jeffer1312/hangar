import { useCallback, useMemo, useRef, useState } from 'react';
import { useFocusEffect } from 'expo-router';
import { dreamFailure, getDreamConfig, getDreamStatus, type DreamConfig, type DreamStatus, type Server } from '@hangar/core';
import { ownServers, useServers } from '../../stores/servers';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

type Data = { configs: Record<string, DreamConfig | null>; dreamerId: string; status: DreamStatus };

/**
 * Configuração, estado e ações do sonho valem na máquina que sonha: o servidor ativo só diz qual é.
 * Relê ao ganhar foco, porque a tela de projeto e a de rodada mudam o estado por baixo.
 */
export function useDream() {
  const all = useServers((s) => s.servers);
  const servers = useMemo(() => ownServers(all), [all]);
  const active = useServers((s) => s.active());
  const [data, setData] = useState<Data | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const operation = useRef(false);

  const load = useCallback(async () => {
    const g = ++generation.current;
    if (!active || active.invite) { setLoading(false); setError(m.dream_load_error()); return; }
    setLoading(true); setError('');
    try {
      const results = await Promise.allSettled(servers.map((s) => getDreamConfig(s)));
      const configs: Record<string, DreamConfig | null> = {};
      servers.forEach((s, i) => { const r = results[i]; configs[s.id] = r.status === 'fulfilled' ? r.value : null; });
      const mine = results[servers.findIndex((s) => s.id === active.id)] as PromiseSettledResult<DreamConfig> | undefined;
      if (!mine || mine.status === 'rejected') throw mine?.reason ?? new Error(active.label);
      const own = mine.value;
      // Máquina escolhida que o app não conhece (ou ainda nenhuma): o ativo responde por ela.
      const dreamer = servers.find((s) => configs[s.id]?.this_machine === own.machine) ?? active;
      const status = await getDreamStatus(dreamer);
      if (g === generation.current) setData({ configs, dreamerId: dreamer.id, status });
    } catch (e) {
      if (g === generation.current) setError(`${m.dream_load_error()} · ${dreamFailure(e)}`);
    } finally {
      if (g === generation.current) setLoading(false);
    }
  }, [active, servers]);

  useFocusEffect(useCallback(() => { void load(); return () => { generation.current++; }; }, [load]));

  const dreamer: Server | null = data ? servers.find((s) => s.id === data.dreamerId) ?? null : null;
  const labelOf = useCallback((machine: string) =>
    servers.find((s) => data?.configs[s.id]?.this_machine === machine)?.label ?? machine, [servers, data]);

  /** Ação da pessoa: uma por vez, toast com vibração no resultado e releitura. Texto devolvido = falha parcial. */
  const act = useCallback(async (action: () => Promise<unknown>, success?: string) => {
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
      void load();
    }
  }, [load]);

  return {
    servers, dreamer, labelOf, act, busy, loading, error, reload: load,
    configs: data?.configs ?? {}, config: data && dreamer ? data.configs[dreamer.id] : null, status: data?.status ?? null,
  };
}
