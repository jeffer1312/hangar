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

type Tier = 'default' | 'priority';
type Catalog = { tier: string | null; supported: boolean; visible: boolean };

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
        next = {
          tier: res.current.service_tier ?? null,
          supported: model?.serviceTiers?.some((tier) => tier.id === 'priority' && tier.hidden !== true) ?? false,
          visible: true,
        };
      } else {
        const res = await getModelOptions(name, server, { fresh: true });
        const model = res.current?.model?.replace(/\[1m\]$/, '');
        next = {
          tier: res.current?.service_tier ?? null,
          supported: res.models.some((mo) => mo.id === model && mo.supports_fast === true),
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
  const on = tier === 'priority';
  const hidden = !candidate || catalog?.visible === false;
  const available = !hidden && !!catalog && !loading && !error && (isCodex
    ? on || (tier === 'default' && catalog.supported)
    : catalog.supported && (tier === 'default' || on));

  const toggle = useCallback(async () => {
    if (busy.current || !available) return;
    if (!server) { setNotice(m.servidor_nao_existe()); return; }
    const my = ++seq.current;
    const revision = liveRevision.current;
    busy.current = true;
    setApplying(true);
    setNotice(null);
    try {
      const res = await setServiceTier(name, on ? 'default' : 'priority', server);
      if (my !== seq.current) return;
      if (!res || res.ok !== true || (res.service_tier !== 'default' && res.service_tier !== 'priority')) {
        setNotice(m.native_invalid_response());
        return;
      }
      setCatalog((cur) => cur ? { ...cur, tier: res.service_tier } : cur);
      if (isCodex && revision === liveRevision.current) setConfirmed({ tier: res.service_tier, before: liveTier });
    } catch (e) {
      if (my === seq.current) setNotice(e instanceof Error ? e.message : String(e));
    } finally {
      if (my === seq.current) { busy.current = false; setApplying(false); }
    }
  }, [available, server, name, on, isCodex, liveTier]);

  return { hidden, on, available, loading, applying, error, notice, load, toggle };
}
