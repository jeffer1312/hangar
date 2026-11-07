import { create } from 'zustand';
import * as SecureStore from 'expo-secure-store';
import { esquecerRota, mergeImportedMachines, reconcilePairEntries, registrarDiag } from '@hangar/core';
import type { ExternalPair, InviteRedeemResult, LanInfo, PairAttach, Server } from '@hangar/core';
import * as m from '../paraglide/messages';
import { readIdentifier } from '../features/machines/api';

const KEY = 'cp_servers_v1';

type Persisted = { servers: Server[]; activeId: string | null };

function isPersisted(value: unknown): value is Persisted {
  if (!value || typeof value !== 'object') return false;
  const p = value as Record<string, unknown>;
  return (p.activeId === null || typeof p.activeId === 'string')
    && Array.isArray(p.servers)
    && p.servers.every((s: unknown) => {
      if (!s || typeof s !== 'object') return false;
      const server = s as Record<string, unknown>;
      if (!['id', 'label', 'baseUrl', 'token'].every((key) => typeof server[key] === 'string')) return false;
      if (!['disabled', 'invite', 'inviteEnded', 'removed', 'ephemeral'].every((key) => server[key] === undefined || typeof server[key] === 'boolean')) return false;
      if (![undefined, 'owner', 'device'].includes(server.credential as string | undefined)) return false;
      if (!['deviceId', 'ownerToken'].every((key) => server[key] === undefined || typeof server[key] === 'string')) return false;
      if (server.connectOff !== undefined && typeof server.connectOff !== 'boolean') return false;
      return ['lan', 'remote', 'connect'].every((key) => {
        if (server[key] === undefined) return true;
        if (!server[key] || typeof server[key] !== 'object') return false;
        const route = server[key] as Record<string, unknown>;
        return typeof route.url === 'string' && typeof route.id === 'string';
      });
    });
}

// O que vem junto da credencial de aparelho não vale para outro token.
function withPlainToken(server: Server, token: string): Server {
  const next = { ...server, token };
  if (server.credential === 'device' || server.connectOff) delete next.disabled;
  delete next.credential;
  delete next.deviceId;
  delete next.connect;
  delete next.ownerToken;
  delete next.connectOff;
  return next;
}

// Depois de voltar à senha ou de passar o foco, ainda chegam 401 de pedidos feitos com o token
// antigo, e `onUnauthorized` não diz qual token foi recusado.
const REVERTED_GRACE_MS = 30_000;

// Por que estes dois avisos existem: as duas formas de perder um servidor eram mudas. A gravação
// no keystore podia falhar (keystore invalidado depois de restaurar backup, disco cheio) com a
// tela seguindo em frente como se tivesse salvo — e no próximo boot o pareamento não estava lá; e
// um 401 apagava o servidor da lista sem uma palavra, deixando "nenhum servidor" no lugar de
// "o token desta máquina não vale mais".
export type AvisoServidores =
  | { tipo: 'persistencia' }
  | { tipo: 'vault_unread' }
  | { tipo: 'identification'; message: string }
  | { tipo: 'token'; label: string };

export interface ServersState {
  servers: Server[];
  activeId: string | null;
  ready: boolean;
  vaultReadable: boolean;
  aviso: AvisoServidores | null;
  load(): Promise<void>;
  add(s: Omit<Server, 'id' | 'label'> & { label?: string }): Server;
  addInvite(result: InviteRedeemResult, activate?: boolean): Server;
  appendImported(servers: Server[]): Server[];
  /** Entrada aprovada pelo Connect: cria a máquina ou renova a que já existe, sem trocar o endereço dela. */
  addConnected(server: Server): Server;
  remove(id: string): void;
  update(id: string, patch: Partial<Pick<Server, 'label' | 'baseUrl'>>): void;
  setActive(id: string): void;
  active(): Server | null;
  /** `rejectedToken`: o token que levou o 401, quando quem chama sabe. */
  markInvalid(id: string, rejectedToken?: string): void;
  reactivateDevice(id: string): void;
  enableConnect(id: string): void;
  ensureActive(id: string): boolean;
  limparAviso(): void;
  rememberLan(id: string, lan: LanInfo, source?: Server): void;
  rememberRemote(id: string, remote: LanInfo, source: Server): void;
  rememberConnect(id: string, connect: LanInfo, source: Server): void;
  rememberDevice(id: string, upgraded: Server, source: Server): void;
  applyPairs(pairs: ExternalPair[]): PairAttach[];
  markEnded(id: string): void;
  ensureOwnActive(): void;
}

// Máquinas desta pessoa: a entrada de convite é da máquina de outra pessoa e não vira escolha.
export const ownServers = (list: Server[]) => list.filter((s) => !s.invite);

export const useServers = create<ServersState>((set, get) => {
  let lastOwnActive: string | null = null;
  let reading: Promise<void> | null = null;
  let readingUserAction = false;
  let writing: Promise<void> | null = null;
  let retriedBeforeWrite = false;
  let vaultFailed = false;
  let writeFailed = false;
  let pending: { update: (p: Persisted) => Persisted; userAction: boolean }[] = [];
  let warnings: AvisoServidores[] = [];
  const ids = new Map<string, string>();
  const naming = new Map<string, AbortController>();
  const revertedUntil = new Map<string, number>();
  const resolveId = (id: string) => ids.get(id) ?? id;

  // Nunca apaga: o 401 pode ser o Python atendendo a porta, `devices.json` ilegível ou um backup
  // restaurado. Sem senha guardada, ou com a guardada também recusada, fica desligada até uma
  // conexão responder.
  function refuseDevice(server: Server) {
    const { ownerToken } = server;
    if (ownerToken) {
      esquecerRota(server.id);
      revertedUntil.set(server.id, Date.now() + REVERTED_GRACE_MS);
      registrarDiag({ evento: 'connect.aparelho_recusado', nivel: 'aviso', tela: 'servidores', operacao: server.id });
      change((p) => ({ ...p, servers: p.servers.map((s) => s.id === server.id ? { ...withPlainToken(s, ownerToken), connectOff: true } : s) }), false);
      return;
    }
    if (server.disabled) return;
    const next = ownServers(get().servers).find((s) => s.id !== server.id && !s.disabled);
    // Pedido atrasado com o token desta entrada leria a máquina que recebeu o foco como a recusada.
    if (next && get().activeId === server.id) revertedUntil.set(next.id, Date.now() + REVERTED_GRACE_MS);
    change((p) => ({
      ...p,
      servers: p.servers.map((s) => s.id === server.id ? { ...s, disabled: true } : s),
      activeId: p.activeId === server.id && next ? next.id : p.activeId,
    }), false);
    warn({ tipo: 'token', label: server.label });
  }

  function warn(aviso: AvisoServidores) {
    const same = (other: AvisoServidores | null) => other?.tipo === aviso.tipo
      && (aviso.tipo !== 'token' || (other.tipo === 'token' && other.label === aviso.label));
    if (same(get().aviso) || warnings.some(same)) return;
    if (get().aviso) warnings.push(aviso);
    else { warnings = []; set({ aviso }); }
  }

  function logFailure(stage: 'read' | 'parse' | 'schema' | 'write', error: unknown) {
    registrarDiag({
      evento: 'servidores.cofre_falhou', nivel: 'erro', tela: 'servidores',
      codigo: stage === 'write' ? 'servers_vault_write_failed' : stage === 'read' ? 'servers_vault_read_failed' : 'servers_vault_corrupt',
      operacao: stage, detalhe: error instanceof SyntaxError ? 'SyntaxError' : error instanceof Error ? 'vault_error' : typeof error,
    });
  }

  function persistir(userAction = true): Promise<void> {
    const { servers: all, activeId } = get();
    const servers = all.filter((s) => !s.ephemeral);
    if (servers.some((s) => s.id === activeId) || activeId === null) lastOwnActive = activeId;
    const salvo: Persisted = { servers, activeId: servers.some((s) => s.id === lastOwnActive) ? lastOwnActive : (servers[0]?.id ?? null) };
    const write = () => SecureStore.setItemAsync(KEY, JSON.stringify(salvo)).then(() => {
      writeFailed = false;
    }).catch((error: unknown) => {
      logFailure('write', error);
      if (userAction || !writeFailed) warn({ tipo: 'persistencia' });
      writeFailed = true;
    });
    // O cofre pode terminar escritas fora de ordem; a mais nova precisa ser a última.
    const current = writing ? writing.then(write) : write();
    writing = current;
    void current.then(() => { if (writing === current) writing = null; });
    return current;
  }

  function change(update: (p: Persisted) => Persisted, userAction = true) {
    if (!get().vaultReadable) pending.push({ update, userAction });
    set(update(get()));
    if (get().vaultReadable) void persistir(userAction);
    else if (reading) readingUserAction ||= userAction;
    else if (!retriedBeforeWrite) {
      retriedBeforeWrite = true;
      void load(userAction);
    } else if (userAction) warn({ tipo: 'vault_unread' });
  }

  function load(userAction = true): Promise<void> {
    if (reading) {
      readingUserAction ||= userAction;
      return reading;
    }
    readingUserAction = userAction;
    const previousWrite = writing;
    reading = Promise.resolve().then(async () => {
      let stage: 'read' | 'parse' | 'schema' = 'read';
      try {
        if (previousWrite) await previousWrite;
        const raw = await SecureStore.getItemAsync(KEY);
        stage = 'parse';
        const p: unknown = raw === null ? { servers: [], activeId: null } : JSON.parse(raw);
        stage = 'schema';
        if (!isPersisted(p)) throw new Error('vault_schema');
        lastOwnActive = p.activeId;
        const ephemeral = get().servers.filter((s) => s.ephemeral && !p.servers.some((saved) => saved.id === s.id));
        const recovered = pending.reduce((value, { update }) => update(value), { servers: [...p.servers, ...ephemeral], activeId: p.activeId });
        const changed = pending.length > 0;
        const manualChange = pending.some((change) => change.userAction);
        pending = [];
        if (recovered.activeId !== null && !recovered.servers.some((s) => s.id === recovered.activeId)) {
          recovered.activeId = ownServers(recovered.servers)[0]?.id ?? null;
        }
        vaultFailed = false;
        retriedBeforeWrite = false;
        warnings = warnings.filter((aviso) => aviso.tipo !== 'vault_unread');
        const aviso = get().aviso?.tipo === 'vault_unread' ? (warnings.shift() ?? null) : get().aviso;
        set({ ...recovered, ready: true, vaultReadable: true, aviso });
        if (changed) await persistir(manualChange);
      } catch (error: unknown) {
        logFailure(stage, error);
        set({ ready: true, vaultReadable: false });
        if (readingUserAction || !vaultFailed) warn({ tipo: 'vault_unread' });
        vaultFailed = true;
      }
    }).finally(() => { reading = null; });
    set({ vaultReadable: false });
    return reading;
  }

  async function resolveLabel(server: Server) {
    const controller = new AbortController();
    naming.set(server.id, controller);
    const isCurrent = () => !controller.signal.aborted && get().vaultReadable
      && get().servers.some((s) => s.id === server.id && s.baseUrl === server.baseUrl && s.token === server.token && s.label === server.label);
    try {
      if (reading) await reading;
      // A leitura pode descobrir um cadastro salvo, que conserva seu apelido.
      if (!isCurrent() || get().servers.find((s) => s.id === server.id) !== server) return;
      const identity = await readIdentifier(server, controller.signal);
      if (reading) await reading;
      if (!isCurrent()) return;
      if (!identity.identificador) throw new Error(m.mobile_machines_invalid_response());
      change((p) => ({ ...p, servers: p.servers.map((s) => s.id === server.id ? { ...s, label: identity.identificador } : s) }), false);
    } catch (error) {
      if (reading) await reading;
      if (isCurrent()) warn({ tipo: 'identification', message: error instanceof Error ? error.message : String(error) });
    } finally {
      if (naming.get(server.id) === controller) naming.delete(server.id);
    }
  }

  return ({
  servers: [],
  activeId: null,
  ready: false,
  vaultReadable: false,
  aviso: null,
  load,
  add({ baseUrl, token, label }) {
    const base = baseUrl.replace(/\/+$/, '');
    // Só máquina própria troca o token: a entrada do par no mesmo endereço tem o token de convidado.
    const existing = get().servers.find((s) => s.baseUrl === base && !s.invite);
    if (existing) naming.get(existing.id)?.abort();
    if (existing && existing.token !== token) esquecerRota(existing.id);
    const renewed = (server: Server) => (server.token === token ? { ...server } : withPlainToken(server, token));
    const s: Server = existing
      ? renewed(existing)
      : { id: `s_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`, label: label ?? m.native_settings_group_server(), baseUrl: base, token };
    change((p) => {
      const saved = p.servers.find((x) => x.baseUrl === base && !x.invite);
      const server = saved ? renewed(saved) : s;
      if (s.id !== server.id) ids.set(s.id, server.id);
      return {
        servers: saved ? p.servers.map((x) => x.id === saved.id ? server : x) : [...p.servers, server],
        activeId: server.id,
      };
    });
    if (!existing && label === undefined) void resolveLabel(s);
    return s;
  },
  appendImported(servers) {
    let added: Server[] = [];
    change((p) => {
      const merged = mergeImportedMachines(p, servers);
      added = merged.added;
      return { servers: merged.servers, activeId: merged.activeId };
    }, false);
    return added;
  },
  addConnected(server) {
    let saved!: Server;
    change((p) => {
      const existing = p.servers.find((s) => !s.invite
        && (s.baseUrl === server.baseUrl || (!!server.lan?.id && s.lan?.id === server.lan.id)));
      if (existing) {
        saved = {
          ...existing, token: server.token, credential: 'device', deviceId: server.deviceId, disabled: false,
          lan: server.lan ?? existing.lan, connect: server.connect ?? existing.connect,
          // A senha que esta entrada usava vira a volta para um 401 do aparelho, como em `upgradeToDevice`.
          ownerToken: existing.credential === 'device' ? existing.ownerToken : existing.token,
        };
        delete saved.connectOff;
      } else {
        // O id vem do nome da máquina, que só é único dentro da conta.
        let id = server.id;
        for (let suffix = 2; p.servers.some((s) => s.id === id); suffix++) id = `${server.id}_${suffix}`;
        saved = { ...server, id };
      }
      return {
        servers: existing ? p.servers.map((s) => s.id === existing.id ? saved : s) : [...p.servers, saved],
        activeId: p.servers.some((s) => s.id === p.activeId && !s.disabled) ? p.activeId : saved.id,
      };
    });
    esquecerRota(saved.id);
    return saved;
  },
  addInvite(result, activate = true) {
    const base = result.address.replace(/\/+$/, '');
    const matches = (server: Server) => {
      try { return new URL(server.baseUrl).origin === new URL(base).origin; }
      catch { return false; }
    };
    const id = `s_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
    let saved!: Server;
    change((p) => {
      const existing = p.servers.find((server) => server.invite && !server.ephemeral && matches(server))
        ?? p.servers.find((server) => server.invite && matches(server));
      const server: Server = {
        ...existing,
        id: existing?.id ?? id,
        label: m.mobile_share_invite_label({ owner: result.owner }),
        baseUrl: base, token: result.token, invite: true, ephemeral: false,
        inviteEnded: false, disabled: false, removed: false,
      };
      saved = server;
      return {
        servers: existing ? p.servers.map((entry) => entry.id === existing.id ? server : entry) : [...p.servers, server],
        activeId: activate ? server.id : p.activeId,
      };
    });
    return saved;
  },
  remove(id) {
    naming.get(resolveId(id))?.abort();
    esquecerRota(resolveId(id));
    change((p) => {
      const target = resolveId(id);
      const servers = p.servers.filter((s) => s.id !== target);
      return { servers, activeId: p.activeId === target ? (ownServers(servers)[0]?.id ?? null) : p.activeId };
    });
  },
  update(id, patch) {
    const target = resolveId(id);
    const servers = get().servers;
    const baseUrl = patch.baseUrl?.replace(/\/+$/, '');
    if (servers.find((s) => s.id === target)?.invite) return;
    if (baseUrl !== undefined && servers.some((s) => s.id !== target && !s.invite && s.baseUrl.replace(/\/+$/, '') === baseUrl)) {
      throw new Error('machine_address_duplicate');
    }
    if (patch.label !== undefined || patch.baseUrl !== undefined) naming.get(target)?.abort();
    change((p) => ({
      ...p,
      servers: p.servers.map((s) => {
        if (s.id !== resolveId(id) || s.invite) return s;
        const server = { ...s, label: patch.label ?? s.label, baseUrl: baseUrl ?? s.baseUrl };
        if (baseUrl !== undefined && baseUrl !== s.baseUrl.replace(/\/+$/, '')) {
          esquecerRota(s.id);
          const identity = s.lan?.id || s.remote?.id;
          if (identity) server.lan = { url: '', id: identity };
          else delete server.lan;
        }
        return server;
      }),
    }));
  },
  setActive(id) {
    change((p) => ({ ...p, activeId: resolveId(id) }));
  },
  active() {
    return get().servers.find((s) => s.id === get().activeId) ?? null;
  },
  markInvalid(id, rejectedToken) {
    const target = get().servers.find((s) => s.id === id);
    // Convite recusado é par/compartilhamento que acabou, não máquina a remover.
    if (target?.invite) { get().markEnded(id); return; }
    const stale = rejectedToken === undefined ? Date.now() < (revertedUntil.get(id) ?? 0) : !!target && rejectedToken !== target.token;
    if (stale) return;
    if (target?.credential === 'device' || target?.connectOff) { refuseDevice(target); return; }
    const label = target?.label ?? '';
    get().remove(id);
    warn({ tipo: 'token', label });
  },
  reactivateDevice(id) {
    const current = get().servers.find((s) => s.id === resolveId(id));
    if (!current?.disabled || (current.credential !== 'device' && !current.connectOff)) return;
    change((p) => ({ ...p, servers: p.servers.map((s) => s.id === current.id ? { ...s, disabled: false } : s) }), false);
  },
  enableConnect(id) {
    change((p) => ({ ...p, servers: p.servers.map((s) => {
      if (s.id !== resolveId(id)) return s;
      const server = { ...s };
      delete server.connectOff;
      return server;
    }) }));
  },
  ensureActive(id) {
    if (!get().servers.some((s) => s.id === id)) return false;
    if (get().activeId !== id) get().setActive(id);
    return true;
  },
  limparAviso() {
    set({ aviso: warnings.shift() ?? null });
  },
  applyPairs(pairs) {
    const { servers, attach } = reconcilePairEntries(get().servers, pairs, () => `p_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`);
    const activeId = servers.some((s) => s.id === get().activeId) ? get().activeId : lastOwnActive;
    set({ servers, activeId });
    return attach;
  },
  ensureOwnActive() {
    const list = get().servers;
    if (!list.find((s) => s.id === get().activeId)?.invite) return;
    const own = list.find((s) => s.id === lastOwnActive && !s.invite) ?? list.find((s) => !s.invite);
    if (own) set({ activeId: own.id });
  },
  markEnded(id) {
    set({ servers: get().servers.map((s) => (s.id === id && s.invite ? { ...s, inviteEnded: true } : s)) });
  },
  rememberLan(id, lan, source) {
    const current = get().servers.find((s) => s.id === resolveId(id));
    if (source && (!current || current.baseUrl !== source.baseUrl || current.token !== source.token)) return;
    change((p) => ({ ...p, servers: p.servers.map((s) => {
      if (s.id !== resolveId(id)) return s;
      const server = { ...s, lan };
      if (server.remote?.id !== lan.id) delete server.remote;
      if (server.connect?.id !== lan.id) delete server.connect;
      return server;
    }) }), false);
  },
  rememberRemote(id, remote, source) {
    const current = get().servers.find((s) => s.id === resolveId(id));
    if (!current || current.invite || current.baseUrl !== source.baseUrl || current.token !== source.token
      || remote.id !== current.lan?.id || current.remote?.url === remote.url && current.remote.id === remote.id) return;
    change((p) => ({ ...p, servers: p.servers.map((s) => s.id === resolveId(id) ? { ...s, remote } : s) }), false);
  },
  rememberConnect(id, connect, source) {
    const current = get().servers.find((s) => s.id === resolveId(id));
    if (!current || current.invite || current.credential !== 'device' || current.baseUrl !== source.baseUrl
      || current.token !== source.token || connect.id !== current.lan?.id
      || current.connect?.url === connect.url && current.connect.id === connect.id) return;
    change((p) => ({ ...p, servers: p.servers.map((s) => s.id === resolveId(id) ? { ...s, connect } : s) }), false);
  },
  rememberDevice(id, upgraded, source) {
    const current = get().servers.find((s) => s.id === resolveId(id));
    if (!current || current.invite || current.baseUrl !== source.baseUrl || current.token !== source.token) return;
    const { token, credential, deviceId, ownerToken } = upgraded;
    change((p) => ({ ...p, servers: p.servers.map((s) => s.id === resolveId(id) ? { ...s, token, credential, deviceId, ownerToken } : s) }), false);
  },
  });
});
