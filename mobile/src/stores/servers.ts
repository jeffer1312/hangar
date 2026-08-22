import { create } from 'zustand';
import * as SecureStore from 'expo-secure-store';
import type { Server } from '@hangar/core';

const KEY = 'cp_servers_v1';

type Persisted = { servers: Server[]; activeId: string | null };

const hostLabel = (u: string) => {
  try {
    return new URL(u).hostname.split('.')[0] || u;
  } catch {
    return u;
  }
};

export interface ServersState {
  servers: Server[];
  activeId: string | null;
  ready: boolean;
  load(): Promise<void>;
  add(s: Omit<Server, 'id' | 'label'> & { label?: string }): Server;
  remove(id: string): void;
  setActive(id: string): void;
  active(): Server | null;
  markInvalid(id: string): void;
}

export const useServers = create<ServersState>((set, get) => ({
  servers: [],
  activeId: null,
  ready: false,
  async load() {
    try {
      const raw = await SecureStore.getItemAsync(KEY);
      const p: Persisted = raw ? JSON.parse(raw) : { servers: [], activeId: null };
      set({ ...p, ready: true });
    } catch {
      // JSON corrompido ou SecureStore falhou: reseta pra vazio e libera a tela
      set({ servers: [], activeId: null, ready: true });
    }
  },
  add({ baseUrl, token, label }) {
    const base = baseUrl.replace(/\/+$/, '');
    const existing = get().servers.find((s) => s.baseUrl === base);
    const s: Server = existing
      ? { ...existing, token }
      : { id: `s_${Date.now().toString(36)}`, label: label ?? hostLabel(base), baseUrl: base, token };
    const servers = existing ? get().servers.map((x) => (x.id === s.id ? s : x)) : [...get().servers, s];
    persist({ servers, activeId: s.id });
    set({ servers, activeId: s.id });
    return s;
  },
  remove(id) {
    const servers = get().servers.filter((s) => s.id !== id);
    const activeId = get().activeId === id ? (servers[0]?.id ?? null) : get().activeId;
    persist({ servers, activeId });
    set({ servers, activeId });
  },
  setActive(id) {
    persist({ servers: get().servers, activeId: id });
    set({ activeId: id });
  },
  active() {
    return get().servers.find((s) => s.id === get().activeId) ?? null;
  },
  markInvalid(id) {
    get().remove(id);
  },
}));

function persist(p: Persisted): Promise<void> {
  return SecureStore.setItemAsync(KEY, JSON.stringify(p)).catch(() => {
    // keystore invalidado pós-restore: escrita falhou silenciosamente antes;
    // agora pelo menos não é fire-and-forget — chamadores podem await e tratar
  });
}
