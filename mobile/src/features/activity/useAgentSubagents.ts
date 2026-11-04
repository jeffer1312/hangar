import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AppState } from 'react-native';
import { useFocusEffect } from 'expo-router';
import { codexSubagentActivityKey, getSubagents, projectCodexSubagent, registrarDiag, type CodexSubagents, type SubagentRun } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { ActivityTimeout, useActivityRequest } from './useActivityRequest';
import * as m from '../../paraglide/messages';

const EMPTY: SubagentRun[] = [];
export type AgentSubagents = ReturnType<typeof useAgentSubagents>;

export function useAgentSubagents(serverId: string | undefined, name: string | undefined, enabled: boolean, conversation?: string, codex = false, states?: CodexSubagents) {
  const server = useServers(state => state.servers.find(item => item.id === serverId));
  const source = JSON.stringify([serverId, name, conversation, codex]);
  const nativeIds = codexSubagentActivityKey(states);
  const refreshActivity = useRef<() => void>(() => {});
  const epoch = useRef({ source, enabled, value: 0 });
  if (epoch.current.source !== source || epoch.current.enabled !== enabled) {
    epoch.current = { source, enabled, value: epoch.current.value + 1 };
  }
  const key = JSON.stringify([source, epoch.current.value]);
  const activeKey = useRef<string | null>(null);
  activeKey.current = enabled ? key : null;
  const [snapshot, setSnapshot] = useState({ key, subs: EMPTY, error: '', loaded: false });
  const [attempt, setAttempt] = useState(0);
  const read = useActivityRequest();
  const retry = useCallback(() => {
    setSnapshot(current => ({ ...current, error: '' }));
    setAttempt(current => current + 1);
  }, []);

  useFocusEffect(useCallback(() => {
    if (!enabled || !server || !name) return;
    let stopped = false;
    let busy = false;
    let rerun = false;
    let retryAfter = 0;
    let timeouts = 0;
    let failureRetry: ReturnType<typeof setTimeout> | undefined;
    // A mesma falha vai ao diário uma vez.
    let logged = '';
    const refresh = async (changed = false) => {
      if (stopped || Date.now() < retryAfter || AppState.currentState !== 'active') return;
      if (busy) { rerun ||= changed; return; }
      clearTimeout(failureRetry);
      busy = true;
      try {
        const subs = await read(getSubagents(name, server));
        if (stopped || activeKey.current !== key) return;
        timeouts = 0;
        retryAfter = 0;
        if (!stopped) setSnapshot(current => {
          if (current.key === key && current.loaded && !current.error && JSON.stringify(current.subs) === JSON.stringify(subs)) return current;
          return { key, subs, error: '', loaded: true };
        });
      } catch (error) {
        if (stopped || activeKey.current !== key) return;
        const timedOut = error instanceof ActivityTimeout;
        if (timedOut) retryAfter = Date.now() + Math.min(60_000, 5_000 * 2 ** Math.min(timeouts++, 4));
        const detail = error instanceof Error ? error.message : String(error);
        if (!timedOut && detail !== logged) {
          logged = detail;
          registrarDiag({ evento: 'activity.subagents_failed', nivel: 'erro', codigo: 'subagents_read_failed', detalhe: detail });
        }
        if (!stopped) setSnapshot(current => ({ key, subs: current.key === key ? current.subs : EMPTY, loaded: current.key === key && current.loaded,
          error: error instanceof ActivityTimeout ? error.message : m.atividade_erro_subagentes() }));
        if (codex) failureRetry = setTimeout(() => void refresh(), Math.max(5_000, retryAfter - Date.now()));
      } finally {
        busy = false;
        if (rerun && !stopped) { rerun = false; void refresh(); }
      }
    };
    refreshActivity.current = () => { void refresh(true); };
    void refresh();
    const timer = codex ? undefined : setInterval(() => { void refresh(); }, 5_000);
    const appState = AppState.addEventListener('change', state => { if (state === 'active') { retryAfter = 0; void refresh(); } });
    return () => { stopped = true; refreshActivity.current = () => {}; clearInterval(timer); clearTimeout(failureRetry); appState.remove(); };
  }, [enabled, server, name, key, read, attempt, codex]));

  const previousActivity = useRef(nativeIds);
  useEffect(() => {
    const changed = previousActivity.current !== nativeIds;
    previousActivity.current = nativeIds;
    if (codex && changed) refreshActivity.current();
  }, [codex, nativeIds]);

  const current = enabled && snapshot.key === key;
  const raw = current ? snapshot.subs : EMPTY;
  const subs = useMemo(() => codex ? raw.map(run => projectCodexSubagent(run, states)) : raw, [raw, codex, states]);
  return { key, subs, error: current ? snapshot.error : '', loaded: current && snapshot.loaded, retry };
}
