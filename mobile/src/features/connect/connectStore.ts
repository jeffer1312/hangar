import { create } from 'zustand';
import { Platform } from 'react-native';
import Constants from 'expo-constants';
import * as SecureStore from 'expo-secure-store';
import { addMachine, changeAccountPassword, endAccountSessions, listMachines, registrarDiag, signIn, signUp, upgradeToDevice } from '@hangar/core';
import type { AccountSession, ConnectMachine, Server } from '@hangar/core';
import { approveDevice, readDevices, reason, removeDevice, type ConnectDevice } from '../machines/api';
import { ownServers, useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';

export const CONNECT_SERVICE_URL = 'https://connect.hangar.dev.br';
const KEY = 'cp_connect_account_v1';
const DEVICES_TIMEOUT_MS = 15_000;

export type ConnectAccount = AccountSession & { name: string };
export type MachineEntry = { phase: 'asking' | 'waiting' | 'failed'; error?: string };
export type DeviceList = { rows: ConnectDevice[] | null; error: string };
type Step = 'signin' | 'signup' | 'password' | 'machine';

export interface ConnectState {
  ready: boolean;
  account: ConnectAccount | null;
  machines: ConnectMachine[] | null;
  machinesError: string;
  loadingMachines: boolean;
  /** Pedido de entrada por `machine_id`. Vive aqui, e não na tela: a aprovação leva até 10 minutos e o token sai uma vez só. */
  entries: Record<string, MachineEntry>;
  devices: Record<string, DeviceList>;
  notice: string;
  load(): Promise<void>;
  signIn(name: string, password: string): Promise<void>;
  signUp(name: string, password: string): Promise<void>;
  signOut(): Promise<void>;
  endSessions(): Promise<void>;
  changePassword(oldPassword: string, newPassword: string): Promise<void>;
  refreshMachines(): Promise<void>;
  enter(machine: ConnectMachine): Promise<Server | null>;
  reenter(server: Server): Promise<void>;
  turnOnConnect(server: Server): Promise<boolean>;
  refreshDevices(): Promise<void>;
  answerDevice(server: Server, deviceId: string, approve: boolean, signal: AbortSignal): Promise<void>;
}

const CODES: Record<string, () => string> = {
  connect_device_denied: m.connect_error_denied,
  connect_approval_expired: m.connect_error_expired,
  connect_token_lost: m.connect_error_token_lost,
  connect_secure_random_unavailable: m.connect_error_secure_random,
  connect_response_invalid: m.mobile_machines_invalid_response,
};

const statusOf = (error: unknown) => (error as { status?: number }).status;

function failure(error: unknown, step: Step): string {
  const known = CODES[(error as { code?: string }).code ?? ''];
  if (known) return known();
  const status = statusOf(error);
  if (status === undefined) return m.connect_error_network();
  if (status === 429) return m.connect_error_limit();
  if (status >= 500) return m.connect_error_unavailable();
  if (status === 422) return m.connect_account_rules();
  if (step === 'signin' && status === 401) return m.connect_error_login();
  if (step === 'signin' && status === 403) return m.connect_error_inactive();
  if (step === 'signup' && status === 409) return m.connect_error_name_taken();
  if (step === 'password' && status === 401) return m.connect_error_wrong_password();
  if (step === 'machine' && status === 409) return m.connect_error_not_linked();
  return m.connect_error_refused();
}

function isAccount(value: unknown): value is ConnectAccount {
  const saved = value as Partial<ConnectAccount> | null;
  return !!saved && typeof saved.serviceUrl === 'string' && typeof saved.token === 'string' && typeof saved.name === 'string';
}

const deviceName = () => (Constants.deviceName || Platform.OS).slice(0, 64);

export function sameMachine(server: Server, machine: ConnectMachine): boolean {
  try { return new URL(server.connect?.url ?? server.baseUrl).hostname === machine.host; }
  catch { return false; }
}

/** Recusada sem senha do dono para onde voltar: só um pedido novo devolve a entrada. */
export const needsEntry = (server: Server) => !!server.disabled && (server.credential === 'device' || !!server.connectOff);

/** Só quem tem credencial de aparelho está numa máquina com Connect. */
export const connectServers = (servers: Server[]) => ownServers(servers).filter((s) => !s.disabled && s.credential === 'device');

export const useConnect = create<ConnectState>((set, get) => {
  let readingDevices = false;

  const setEntry = (id: string, entry: MachineEntry | null) => set((state) => {
    const entries = { ...state.entries };
    if (entry) entries[id] = entry;
    else delete entries[id];
    return { entries };
  });

  const signedOut = (notice: string) => set({ account: null, machines: null, machinesError: '', loadingMachines: false, notice });

  // O token já não vale: a falha ao apagar só deixa um token morto, que o próximo 401 limpa.
  async function dropAccount(notice: string) {
    await SecureStore.deleteItemAsync(KEY).catch(() => {
      registrarDiag({ evento: 'connect.conta_nao_apagada', nivel: 'aviso', codigo: 'connect_account_delete_failed' });
    });
    signedOut(notice);
  }

  async function openAccount(login: typeof signIn, step: Step, rawName: string, password: string) {
    const name = rawName.trim();
    let session: AccountSession;
    try { session = await login(CONNECT_SERVICE_URL, name, password); }
    catch (error) { throw new Error(failure(error, step)); }
    const account = { ...session, name };
    try { await SecureStore.setItemAsync(KEY, JSON.stringify(account)); }
    catch { throw new Error(m.connect_error_vault()); }
    set({ account, machines: null, machinesError: '', notice: '' });
    void get().refreshMachines();
  }

  return {
    ready: false,
    account: null,
    machines: null,
    machinesError: '',
    loadingMachines: false,
    entries: {},
    devices: {},
    notice: '',
    async load() {
      if (get().ready) return;
      try {
        const raw = await SecureStore.getItemAsync(KEY);
        const saved: unknown = raw === null ? null : JSON.parse(raw);
        set({ ready: true, account: isAccount(saved) ? saved : null });
      } catch {
        registrarDiag({ evento: 'connect.conta_ilegivel', nivel: 'aviso', codigo: 'connect_account_unreadable' });
        set({ ready: true, notice: m.connect_error_vault() });
      }
    },
    signIn: (name, password) => openAccount(signIn, 'signin', name, password),
    signUp: (name, password) => openAccount(signUp, 'signup', name, password),
    async signOut() {
      try { await SecureStore.deleteItemAsync(KEY); }
      catch { throw new Error(m.connect_error_vault()); }
      signedOut('');
    },
    async endSessions() {
      const { account } = get();
      if (!account) return;
      try { await endAccountSessions(account); }
      catch (error) {
        // 401: esta sessão já tinha acabado, então o pedido cumpriu o que importa aqui.
        if (statusOf(error) !== 401) throw new Error(failure(error, 'machine'));
      }
      await dropAccount(m.connect_sessions_ended());
    },
    async changePassword(oldPassword, newPassword) {
      const { account } = get();
      if (!account) return;
      try { await changeAccountPassword(account, oldPassword, newPassword); }
      catch (error) { throw new Error(failure(error, 'password')); }
    },
    async refreshMachines() {
      const { account } = get();
      if (!account) return;
      set({ loadingMachines: true, machinesError: '' });
      try {
        const machines = await listMachines(account);
        if (get().account?.token === account.token) set({ machines, loadingMachines: false });
      } catch (error) {
        if (get().account?.token !== account.token) return;
        if (statusOf(error) === 401) await dropAccount(m.connect_session_ended());
        else set({ loadingMachines: false, machinesError: failure(error, 'machine') });
      }
    },
    async enter(machine) {
      const { account, entries } = get();
      const phase = entries[machine.machine_id]?.phase;
      if (!account || !machine.online || phase === 'asking' || phase === 'waiting') return null;
      setEntry(machine.machine_id, { phase: 'asking' });
      try {
        const server = await addMachine(account, machine, deviceName(), () => setEntry(machine.machine_id, { phase: 'waiting' }));
        const saved = useServers.getState().addConnected(server);
        setEntry(machine.machine_id, null);
        return saved;
      } catch (error) {
        setEntry(machine.machine_id, { phase: 'failed', error: failure(error, 'machine') });
        // O 401 pode ser do bilhete, na máquina, ou da conta, no serviço: reler a lista diz qual foi.
        if (statusOf(error) === 401) void get().refreshMachines();
        return null;
      }
    },
    async reenter(server) {
      await get().load();
      if (!get().account) return;
      set({ notice: '' });
      await get().refreshMachines();
      const machine = get().machines?.find((candidate) => sameMachine(server, candidate));
      if (!get().account || get().machinesError) return;
      if (!machine) set({ notice: m.connect_reenter_unknown({ name: server.label }) });
      else if (!machine.online) set({ notice: m.connect_reenter_offline({ name: server.label }) });
      else await get().enter(machine);
    },
    async turnOnConnect(server) {
      useServers.getState().enableConnect(server.id);
      const current = useServers.getState().servers.find((s) => s.id === server.id);
      if (!current) return false;
      const upgraded = await upgradeToDevice(current);
      if (upgraded.credential !== 'device') return false;
      useServers.getState().rememberDevice(current.id, upgraded, current);
      return true;
    },
    async refreshDevices() {
      if (readingDevices) return;
      readingDevices = true;
      try {
        await Promise.all(connectServers(useServers.getState().servers).map(async (server) => {
          try {
            const rows = await readDevices(server, AbortSignal.timeout(DEVICES_TIMEOUT_MS));
            set((state) => ({ devices: { ...state.devices, [server.id]: { rows, error: '' } } }));
          } catch (error) {
            set((state) => ({ devices: { ...state.devices, [server.id]: { rows: state.devices[server.id]?.rows ?? null, error: reason(error) } } }));
          }
        }));
      } finally {
        readingDevices = false;
      }
    },
    async answerDevice(server, deviceId, approve, signal) {
      await (approve ? approveDevice : removeDevice)(server, deviceId, signal);
      await get().refreshDevices();
    },
  };
});
