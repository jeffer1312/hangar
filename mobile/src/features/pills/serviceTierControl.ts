import { useCallback, useEffect, useRef, useState } from 'react';
import { getCodexModels, getModelOptions, parseStatusLine, setServiceTier } from '@hangar/core';
import { chatStore } from '../../stores/chat';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';

interface Args {
  serverId: string;
  name: string;
  provider: string | null;
  model: string | null;
  open: boolean;
}

export type Tier = 'default' | 'priority' | 'ultrafast';
// `ultrafast`: o modelo atual também aceita Ultra Fast (Codex Pro; motor com conta fixa Pro).
type Catalog = { tier: string | null; supported: boolean; ultrafast: boolean; visible: boolean };
/// As três velocidades, na ordem do seletor.
export const SPEED_TIERS = ['default', 'priority', 'ultrafast'] as const satisfies readonly Tier[];
const TIERS: readonly string[] = SPEED_TIERS;
export const speedLabel = (tier: Tier) =>
  tier === 'ultrafast' ? m.native_ctl_speed_ultrafast() : tier === 'priority' ? m.native_ctl_fast() : m.native_ctl_speed_normal();
export const speedHint = (tier: Tier) =>
  tier === 'ultrafast' ? m.native_ctl_speed_ultrafast_hint() : tier === 'priority' ? m.native_ctl_speed_fast_hint() : m.native_ctl_speed_normal_hint();

export function useServiceTierControl({ serverId, name, provider, model: selectedModel, open }: Args) {
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  const session = useSessions((s) => s.byServerRecord?.[serverId]?.find((x) => x.name === name)
    ?? s.rows.find((x) => x.serverId === serverId && x.name === name) ?? null);
  const chat = chatStore(serverId, name);
  const statusLine = chat.use((s) => s.statusLine);
  const statusModel = parseStatusLine(statusLine, session)?.model ?? null;
  const listedModel = session?.model ?? null;
  const sseTier = chat.use((s) => s.stateEvent?.codex_service_tier ?? null);
  const liveTier = sseTier ?? session?.codex_service_tier ?? null;
  const engine = session?.engine;
  const isCodex = provider === 'codex';
  const candidate = isCodex || ((!provider || provider === 'claude') && !!engine);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [confirmed, setConfirmed] = useState<{ tier: Tier; before: string | null } | null>(null);
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const busy = useRef(false);
  const reading = useRef(false);
  const seq = useRef(0);
  const liveRevision = useRef(0);

  useEffect(() => {
    seq.current++;
    busy.current = false;
    reading.current = false;
    setCatalog(null);
    setConfirmed(null);
    setLoading(false);
    setApplying(false);
    setError(null);
    setNotice(null);
    return () => { seq.current++; };
  }, [server, name, provider, engine, listedModel, statusModel, selectedModel]);

  // O ACK cobre o atraso do SSE, mas não pode vencer uma mudança posterior no terminal.
  useEffect(() => {
    liveRevision.current++;
    setConfirmed(null);
  }, [liveTier]);

  const load = useCallback(async () => {
    if (!candidate || (busy.current && !reading.current)) return;
    if (!server) { setError(m.servidor_nao_existe()); return; }
    const my = ++seq.current;
    busy.current = true;
    reading.current = true;
    setLoading(true);
    setError(null);
    setNotice(null);
    try {
      let next: Catalog;
      if (isCodex) {
        const res = await getCodexModels(name, server);
        const model = res.models.find((mo) => mo.model === res.current.model);
        const offers = (id: string) => model?.serviceTiers?.some((tier) => tier.id === id && tier.hidden !== true) ?? false;
        next = {
          tier: res.current.service_tier ?? null,
          supported: offers('priority'),
          ultrafast: offers('ultrafast'),
          visible: true,
        };
      } else {
        const res = await getModelOptions(name, server, { fresh: true });
        const model = res.current?.model?.replace(/\[1m\]$/, '');
        next = {
          tier: res.current?.service_tier ?? null,
          supported: res.models.some((mo) => mo.id === model && mo.supports_fast === true),
          ultrafast: res.models.some((mo) => mo.id === model && mo.supports_ultrafast === true),
          visible: res.kind === 'engine' && res.engine === engine && res.supports_fast === true,
        };
      }
      if (my === seq.current) { setCatalog(next); setConfirmed(null); }
    } catch (e) {
      if (my === seq.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (my === seq.current) { busy.current = false; reading.current = false; setLoading(false); }
    }
  }, [candidate, server, name, isCodex, engine]);

  useEffect(() => { if (open) void load(); }, [open, load, listedModel, statusModel, selectedModel]);

  const tier = isCodex
    ? (confirmed && confirmed.before === liveTier ? confirmed.tier : liveTier ?? catalog?.tier)
    : catalog?.tier;
  const known = tier === 'priority' || tier === 'ultrafast';
  const current: Tier = known ? tier : 'default';
  const hidden = !candidate || catalog?.visible === false;
  const available = !hidden && !!catalog && !loading && !error && (isCodex
    ? known || (tier === 'default' && (catalog.supported || catalog.ultrafast))
    : catalog.supported && (tier === 'default' || known));
  // Ultra Fast já ligado continua escolhível para poder desligar.
  const ultrafast = available && (current === 'ultrafast' || catalog?.ultrafast === true);

  const choose = useCallback(async (next: Tier) => {
    if (busy.current || !available || next === current || (next === 'ultrafast' && !ultrafast)) return;
    if (!server) { setNotice(m.servidor_nao_existe()); return; }
    const my = ++seq.current;
    const revision = liveRevision.current;
    busy.current = true;
    setApplying(true);
    setNotice(null);
    try {
      const res = await setServiceTier(name, next, server);
      if (my !== seq.current) return;
      // Vale a velocidade que o servidor confirmou (como no nativo), desde que seja uma das três.
      if (!res || res.ok !== true || !TIERS.includes(res.service_tier)) {
        setNotice(m.native_invalid_response());
        return;
      }
      const applied = res.service_tier as Tier;
      setCatalog((cur) => cur ? { ...cur, tier: applied } : cur);
      if (isCodex && revision === liveRevision.current) setConfirmed({ tier: applied, before: liveTier });
    } catch (e) {
      if (my === seq.current) setNotice(e instanceof Error ? e.message : String(e));
    } finally {
      if (my === seq.current) { busy.current = false; setApplying(false); }
    }
  }, [available, ultrafast, server, name, current, isCodex, liveTier]);

  return { hidden, tier: current, ultrafast, available, loading, applying, error, notice, load, choose };
}
