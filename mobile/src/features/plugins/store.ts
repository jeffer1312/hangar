import { create } from 'zustand';
import {
  checkPluginsForServer, formataErro, getAvailablePluginsForServer, getPluginsForServer, isSessionBusyRefusal, pluginActionForServer,
  recarregarSessao, removeMarketplaceForServer, updateMarketplaceForServer,
  type AvailablePlugin, type PluginAction, type PluginsChange, type PluginsState, type Server,
} from '@hangar/core';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

/** Frase do código do servidor; sem envelope, a mensagem do pedido (rede, prazo). */
export function pluginsErrorText(e: unknown): string {
  const { envelope, status, code } = (e ?? {}) as { envelope?: unknown; status?: number; code?: string };
  // 404 sem código é servidor que ainda não tem as rotas de plugins.
  if (status === 404 && !code) return m.plugins_server_outdated();
  return formataErro(envelope) ?? (e instanceof Error ? e.message : String(e));
}

/** `text`: o que mudou (null = nada mudou); `note`: resultado que sempre vira aviso curto. */
export interface Outcome { text: string | null; note?: string }

const keyOf = (s: Server) => `${s.id}|${s.baseUrl}|${s.token}`;

// Lista e detalhe são rotas diferentes e mostram o mesmo estado: a mudança feita no detalhe aparece
// na lista e no aviso de reabrir ao voltar.
interface PluginsStore {
  serverKey: string;
  state: PluginsState | null;
  loading: boolean;
  error: string;
  /** Uma mudança por vez: o servidor recusa a segunda (409 `plugins_busy`). */
  busy: string | null;
  available: AvailablePlugin[] | null;
  availableLoading: boolean;
  availableError: string;
  /** O que mudou por último, enquanto há sessão Claude aberta com o estado anterior. */
  changed: string | null;
  reopening: boolean;
  refused: string[];
  reset: (s: Server) => void;
  load: (s: Server) => Promise<void>;
  loadAvailable: (s: Server) => Promise<void>;
  check: (s: Server) => Promise<void>;
  change: (s: Server, busy: string, run: () => Promise<PluginsChange>, describe: (c: PluginsChange) => Outcome, liveCount: number) => Promise<boolean>;
  reopen: (s: Server, names: string[]) => Promise<void>;
  dismiss: () => void;
}

export const usePlugins = create<PluginsStore>((set, get) => {
  const same = (s: Server) => get().serverKey === keyOf(s);
  // Leitura que volta depois de uma mudança ou conferência traria o disco de antes: só vale a mais recente.
  let stateSeq = 0;
  let availableSeq = 0;
  return {
    serverKey: '',
    state: null,
    loading: false,
    error: '',
    busy: null,
    available: null,
    availableLoading: false,
    availableError: '',
    changed: null,
    reopening: false,
    refused: [],
    reset: (s) => {
      if (same(s)) return;
      set({ serverKey: keyOf(s), state: null, loading: false, error: '', busy: null, available: null,
        availableLoading: false, availableError: '', changed: null, reopening: false, refused: [] });
    },
    load: async (s) => {
      // Com uma mudança em curso, a leitura poderia chegar depois dela com o disco de antes.
      if (get().busy) return;
      const n = ++stateSeq;
      const current = () => same(s) && n === stateSeq;
      set({ loading: true, error: '' });
      try {
        const state = await getPluginsForServer(s);
        if (current()) set({ state });
      } catch (e) {
        if (current()) set({ error: pluginsErrorText(e) });
      } finally {
        if (current()) set({ loading: false });
      }
    },
    loadAvailable: async (s) => {
      const n = ++availableSeq;
      const current = () => same(s) && n === availableSeq;
      set({ availableLoading: true, availableError: '' });
      try {
        const { plugins } = await getAvailablePluginsForServer(s);
        if (current()) set({ available: plugins });
      } catch (e) {
        if (current()) set({ availableError: pluginsErrorText(e) });
      } finally {
        if (current()) set({ availableLoading: false });
      }
    },
    check: async (s) => {
      if (get().busy) return;
      stateSeq++;
      availableSeq++;
      set({ busy: 'check', loading: false });
      try {
        const state = await checkPluginsForServer(s);
        if (same(s)) set({ state, available: null, availableLoading: false, availableError: '' });
      } catch (e) {
        if (same(s)) toast.erro(pluginsErrorText(e));
      } finally {
        if (same(s)) set({ busy: null });
      }
    },
    change: async (s, busy, run, describe, liveCount) => {
      if (get().busy) return false;
      // Atualizar sozinho (`mkt:<nome>:auto`) não muda o que o Descobrir oferece.
      const mkt = busy.startsWith('mkt:') && !busy.endsWith(':auto');
      stateSeq++;
      if (mkt) availableSeq++;
      set({ busy, loading: false });
      try {
        const { result, ...state } = await run();
        if (!same(s)) return false;
        // Marketplace novo, removido ou atualizado muda o que o Descobrir oferece.
        set(mkt ? { state, available: null, availableLoading: false, availableError: '' } : { state });
        const { text, note } = describe({ ...state, result });
        if (note) toast.ok(note);
        if (text && liveCount > 0) {
          if (!note) haptic.success();
          set({ changed: text, refused: [] });
        } else if (text && !note) toast.ok(text);
        return true;
      } catch (e) {
        if (same(s)) toast.erro(pluginsErrorText(e));
        return false;
      } finally {
        if (same(s)) set({ busy: null });
      }
    },
    reopen: async (s, names) => {
      const was = get().changed;
      set({ reopening: true, refused: [] });
      const refused: string[] = [];
      // Uma por vez: reabrir várias juntas disputaria a mesma conta.
      for (const name of names) {
        try {
          await recarregarSessao(name, s);
        } catch (e) {
          refused.push(isSessionBusyRefusal(e) ? m.plugins_reopen_refused({ name }) : `${name}: ${pluginsErrorText(e)}`);
        }
      }
      if (!same(s)) return;
      // Mudança feita durante a reabertura não chegou às sessões: o aviso dela fica.
      set({ reopening: false, refused, changed: refused.length || get().changed !== was ? get().changed : null });
      if (refused.length) haptic.error();
      else haptic.success();
    },
    dismiss: () => set({ changed: null, refused: [] }),
  };
});

/** Ligar, desligar, atualizar, remover ou instalar um plugin, com o texto do que mudou. */
export function runPluginAction(s: Server, p: { id: string; name: string; update_known?: boolean; available_version?: string | null }, action: PluginAction, liveCount: number) {
  return usePlugins.getState().change(s, `${p.id}:${action}`, () => pluginActionForServer(s, p.id, action), ({ result, plugins }) => {
    const name = p.name;
    if (action === 'enable') return { text: m.plugins_action_enabled({ name }) };
    if (action === 'disable') return { text: m.plugins_action_disabled({ name }) };
    if (action === 'uninstall') return { text: m.plugins_action_removed({ name }) };
    if (action === 'install') return { text: m.plugins_action_installed({ name }) };
    if (result?.outcome === 'up_to_date') return { text: null, note: m.plugins_update_up_to_date({ name }) };
    const version = result?.new_version ?? plugins.find((x) => x.id === p.id)?.version ?? p.available_version ?? '';
    const text = m.plugins_action_updated({ name, version });
    // Fonte externa: a versão nova só se sabe agora, então o resultado sempre aparece.
    return p.update_known === false ? { text, note: m.plugins_update_done({ name, version }) } : { text };
  }, liveCount);
}

export function runMarketplaceAction(s: Server, name: string, action: 'update' | 'remove', liveCount: number) {
  return usePlugins.getState().change(s, `mkt:${name}:${action}`,
    () => action === 'update' ? updateMarketplaceForServer(s, name) : removeMarketplaceForServer(s, name),
    () => ({ text: action === 'update' ? m.plugins_action_marketplace_updated({ name }) : m.plugins_action_marketplace_removed({ name }) }), liveCount);
}
