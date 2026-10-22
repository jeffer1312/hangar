import { useCallback, useEffect, useRef, useState } from 'react';
import { AppState } from 'react-native';
import { Aquecendo, fetchCostsForServer, fetchUsoForServer, motivoDoServidor, type CostReport, type Server, type UsoFiltros, type UsoReport } from '@hangar/core';
import * as m from '../../paraglide/messages';

export type ReportKind = 'costs' | 'usage';
type Reports = { costs: CostReport; usage: UsoReport };
export interface ReportPart<R> { id: string; label: string; report: Partial<R> | null; pending: boolean; error?: string; warming?: { read: number; total: number } }
// A Home desmonta ao abrir o teclado; o resumo conserva também os avisos de leitura parcial.
let summaryCache: { key: string; at: number; parts: ReportPart<CostReport>[] } | undefined;

export function useReports<K extends ReportKind>(kind: K, servers: Server[], period: string, filters: UsoFiltros, active: boolean, summary = false) {
  const [foreground, setForeground] = useState(AppState.currentState === 'active');
  useEffect(() => {
    const subscription = AppState.addEventListener('change', (state) => setForeground(state === 'active'));
    return () => subscription.remove();
  }, []);
  const chosen = servers.filter((s) => !s.invite && !s.disabled && !s.ephemeral).sort((a, b) => a.id.localeCompare(b.id));
  const key = JSON.stringify([kind, chosen, period, filters, summary]);
  const [attempt, setAttempt] = useState(0);
  const fresh = useRef(false);
  const refresh = useCallback(() => { fresh.current = true; setAttempt((n) => n + 1); }, []);
  const requestKey = JSON.stringify([key, active, foreground, attempt]);
  const [state, setState] = useState<{ key: string; parts: ReportPart<Reports[K]>[] }>({ key: '', parts: [] });

  useEffect(() => {
    if (!active || !foreground) return;
    let alive = true;
    const timers = new Set<ReturnType<typeof setTimeout>>();
    const force = fresh.current;
    fresh.current = false;
    if (kind === 'costs' && summary && !force && summaryCache?.key === key && Date.now() - summaryCache.at < 60_000) {
      setState({ key: requestKey, parts: summaryCache.parts as ReportPart<Reports[K]>[] });
      return;
    }
    setState({ key: requestKey, parts: chosen.map((s) => ({ id: s.id, label: s.label, report: null, pending: true })) });
    const update = (server: Server, patch: Partial<ReportPart<Reports[K]>>) => {
      if (alive) setState((previous) => {
        if (previous.key !== requestKey) return previous;
        const parts = previous.parts.map((part) => part.id === server.id ? { ...part, ...patch } : part);
        if (kind === 'costs' && summary && parts.every((part) => !part.pending) && parts.some((part) => part.report && !part.error)) {
          summaryCache = { key, at: Date.now(), parts: parts as ReportPart<CostReport>[] };
        }
        return { key: requestKey, parts };
      });
    };
    const read = async (server: Server, tries: number) => {
      try {
        const report = (kind === 'costs'
          ? await fetchCostsForServer(server, period, force, summary)
          : await fetchUsoForServer(server, period, filters, force)) as Partial<Reports[K]>;
        if (!alive) return;
        if (!report || !report.totals || typeof report.totals !== 'object') throw new Error(m.home_usage_load_failed());
        if (kind === 'usage') {
          const usage = report as Partial<UsoReport>;
          const mismatched = (['conta', 'projeto', 'modelo', 'plugin'] as const).some((field) => {
            if (!filters[field]?.length) return false;
            const wanted = new Set(filters[field]);
            const applied = new Set(Array.isArray(usage[field]) ? usage[field] : []);
            return wanted.size !== applied.size || [...wanted].some((value) => !applied.has(value));
          }) || !!filters.foco && usage.foco !== filters.foco;
          if (mismatched) {
            update(server, { report: null, pending: false, warming: undefined, error: m.mobile_usage_filters_unsupported() });
            return;
          }
        }
        update(server, { report, pending: false, warming: undefined,
          error: report.applied?.period === period ? undefined : m.home_usage_period_unsupported() });
      } catch (error) {
        if (!alive) return;
        if (error instanceof Aquecendo && tries < 99) {
          update(server, { warming: { read: error.lidos, total: error.total } });
          const timer = setTimeout(() => { timers.delete(timer); void read(server, tries + 1); }, 3000);
          timers.add(timer);
        } else update(server, { report: null, pending: false, warming: undefined,
          error: error instanceof Aquecendo ? m.home_usage_warming_timeout() : motivoDoServidor(error) ?? m.native_network_error() });
      }
    };
    for (const server of chosen) void read(server, 0);
    // As APIs públicas não recebem AbortSignal; a geração invalida a resposta que já saiu.
    return () => { alive = false; for (const timer of timers) clearTimeout(timer); };
  }, [requestKey]);

  const parts = state.key === requestKey && active && foreground ? state.parts : [];
  return { parts, loading: active && foreground && (state.key !== requestKey || parts.some((p) => p.pending)),
    accepted: parts.some((p) => p.report && !p.error), refresh };
}
