import { create } from 'zustand';
import { Platform } from 'react-native';
import Constants from 'expo-constants';
import * as SecureStore from 'expo-secure-store';
import { addMachine, changeAccountPassword, endAccountSessions, installCode, listMachines, registrarDiag, signIn, signUp, upgradeToDevice } from '@hangar/core';
import type { AccountSession, ConnectMachine, Server } from '@hangar/core';
import { approveDevice, readDevices, reason, removeDevice, type ConnectDevice } from '../machines/api';
import { ownServers, useServers } from '../../stores/servers';
import type { GroupTarget } from './pendingGroups';
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
  /** Código curto deste celular, repetido no cartão de quem aprova; vazio sem id guardado. */
  installCode: string;
  /** Por que o código deste celular não está disponível (cofre ou falta de gerador seguro); vazio quando está. */
  installCodeError: string;
  load(): Promise<void>;
  loadInstallCode(): Promise<void>;
  signIn(name: string, password: string): Promise<void>;
  signUp(name: string, password: string): Promise<void>;
  signOut(): Promise<void>;
  endSessions(): Promise<void>;
  changePassword(oldPassword: string, newPassword: string): Promise<void>;
  refreshMachines(): Promise<void>;
  enter(machine: ConnectMachine): Promise<Server | null>;
  /** Pede entrada em todas as máquinas online ainda fora deste aparelho, com um cartão só em quem aprova. */
  enterAll(): Promise<void>;
  reenter(server: Server): Promise<void>;
  turnOnConnect(server: Server): Promise<boolean>;
  refreshDevices(): Promise<void>;
  answerDevice(server: Server, deviceId: string, approve: boolean, signal: AbortSignal): Promise<void>;
  /** Responde o mesmo celular em várias máquinas, cada uma pela própria rota; devolve as que falharam, com o motivo. */
  answerGroup(targets: GroupTarget[], approve: boolean): Promise<{ label: string; error: string }[]>;
}

const CODES: Record<string, () => string> = {
  connect_device_denied: m.connect_error_denied,
  connect_approval_expired: m.connect_error_expired,
  connect_token_lost: m.connect_error_token_lost,
  connect_secure_random_unavailable: m.connect_error_secure_random,
  connect_response_invalid: m.mobile_machines_invalid_response,
  connect_install_id_unreadable: m.connect_error_install_id,
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

const INSTALL_KEY = 'cp_connect_install_id_v1';
// O mesmo valor em todos os pedidos deste aparelho: só junta o cartão de aprovação, nunca dá acesso.
// Sem `id`, `missing` diz o motivo, para a tela: cofre ou falta de gerador seguro.
type InstallId = { id: string; missing?: undefined } | { id?: undefined; missing: string };
let reading: Promise<InstallId> | null = null;
// "Entrar em todas" pede o id várias vezes de uma vez: uma leitura só, senão cada pedido nasceria com o seu.
const installId = () => (reading ??= readInstallId().finally(() => { reading = null; }));
// Cofre que não responde deixaria a máquina em "pedindo" para sempre.
const INSTALL_VAULT_MS = 5_000;
async function withinVaultTime<T>(work: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([work, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('timeout')), INSTALL_VAULT_MS); })]);
  } finally { clearTimeout(timer); }
}
// A mesma regra do servidor (`redeem_from` em `hangar-connect`).
const validInstallId = (id: string) => /^[A-Za-z0-9_-]{1,64}$/.test(id);
async function readInstallId(): Promise<InstallId> {
  let saved: string | null;
  try { saved = await withinVaultTime(SecureStore.getItemAsync(INSTALL_KEY)); }
  catch {
    // Gerar outro aqui trocaria o código que a pessoa já conferiu e separaria este celular em dois cartões: o pedido para
    // com erro e "tentar de novo" relê o mesmo cofre.
    registrarDiag({ evento: 'connect.instalacao_ilegivel', nivel: 'aviso', codigo: 'connect_install_id_unreadable' });
    throw Object.assign(new Error(m.connect_error_install_id()), { code: 'connect_install_id_unreadable' });
  }
  if (saved && validInstallId(saved)) return { id: saved };
  // Só aqui gerar outro é certo: o servidor recusaria este em todo pedido.
  if (saved) registrarDiag({ evento: 'connect.instalacao_invalida', nivel: 'aviso', codigo: 'connect_install_id_invalid' });
  if (!globalThis.crypto?.getRandomValues) {
    registrarDiag({ evento: 'connect.instalacao_sem_aleatorio', nivel: 'aviso', codigo: 'connect_install_id_no_random' });
    return { missing: m.connect_install_code_no_random() };
  }
  const id = Array.from(globalThis.crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, '0')).join('');
  // Sem cofre, os pedidos saem sem o id: cada máquina mostra o seu cartão, como antes.
  try { await withinVaultTime(SecureStore.setItemAsync(INSTALL_KEY, id)); }
  catch {
    registrarDiag({ evento: 'connect.instalacao_nao_gravada', nivel: 'aviso', codigo: 'connect_install_id_unsaved' });
    return { missing: m.connect_install_code_unavailable() };
  }
  return { id };
}

/** Máquinas da conta ligadas e ainda fora deste aparelho: a tela e "Entrar em todas" usam a mesma conta. */
export function missingMachines(machines: ConnectMachine[] | null, servers: Server[]): ConnectMachine[] {
  const own = ownServers(servers);
  return (machines ?? []).filter((machine) => machine.online && !own.some((server) => sameMachine(server, machine)));
}

export function sameMachine(server: Server, machine: ConnectMachine): boolean {
  if (server.connectHost === machine.host) return true;
  try { return new URL(server.connect?.url ?? server.baseUrl).hostname === machine.host; }
  catch { return false; }
}

const sameRow = (a: ConnectMachine, b: ConnectMachine) =>
  a.machine_id === b.machine_id && a.name === b.name && a.host === b.host && a.online === b.online;

/** Recusada sem senha do dono para onde voltar: só um pedido novo devolve a entrada. */
export const needsEntry = (server: Server) => !!server.disabled && (server.credential === 'device' || !!server.connectOff);

/** Só quem tem credencial de aparelho está numa máquina com Connect. */
export const connectServers = (servers: Server[]) => ownServers(servers).filter((s) => !s.disabled && s.credential === 'device');

export const useConnect = create<ConnectState>((set, get) => {
  let devicesRead: Promise<void> | null = null;
  let readAgain = false;
  let machinesRead = 0;
  let newestMachines: Promise<void> = Promise.resolve();

  async function readMachines(account: ConnectAccount) {
    const read = ++machinesRead;
    // Só a leitura mais nova da mesma conta escreve: uma resposta atrasada não desfaz a recente.
    const current = () => read === machinesRead && get().account?.token === account.token;
    set({ loadingMachines: true, machinesError: '' });
    try {
      const fresh = await listMachines(account);
      if (!current()) return;
      const before = get().machines ?? [];
      const machines = fresh.map((row) => before.find((old) => sameRow(old, row)) ?? row);
      set({ machines, loadingMachines: false });
    } catch (error) {
      if (!current()) return;
      if (statusOf(error) === 401) await dropAccount(m.connect_session_ended());
      else set({ loadingMachines: false, machinesError: failure(error, 'machine') });
    }
  }

  const setEntry = (id: string, entry: MachineEntry | null) => set((state) => {
    const entries = { ...state.entries };
    if (entry) entries[id] = entry;
    else delete entries[id];
    return { entries };
  });

  const signedOut = (notice: string) => set({ account: null, machines: null, machinesError: '', loadingMachines: false, entries: {}, devices: {}, notice });

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
    set({ account, machines: null, machinesError: '', entries: {}, devices: {}, notice: '' });
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
    installCode: '',
    installCodeError: '',
    async loadInstallCode() {
      // A causa já foi ao diário em `readInstallId`.
      const { id, missing } = await installId().catch((): InstallId => ({ missing: m.connect_install_code_unavailable() }));
      set({ installCode: id ? installCode(id) : '', installCodeError: missing ?? '' });
    },
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
      if (!account) throw new Error(m.connect_session_ended());
      try { await changeAccountPassword(account, oldPassword, newPassword); }
      catch (error) { throw new Error(failure(error, 'password')); }
    },
    async refreshMachines() {
      const { account } = get();
      if (!account) return;
      const own = readMachines(account);
      newestMachines = own;
      // Quem espera (o puxar para atualizar) só termina com a leitura mais nova, não com a sua já superada.
      let seen: Promise<void>;
      do { seen = newestMachines; await seen; } while (seen !== newestMachines);
    },
    async enter(machine) {
      const { account, entries } = get();
      const phase = entries[machine.machine_id]?.phase;
      if (!account || !machine.online || phase === 'asking' || phase === 'waiting') return null;
      setEntry(machine.machine_id, { phase: 'asking' });
      // A entrada é da conta que pediu: trocada no meio da espera, o resultado não marca a conta nova.
      const sameAccount = () => get().account?.token === account.token;
      try {
        const install = await installId();
        // Leitura boa depois de uma que falhou: o código volta à tela, o aviso sai.
        if (install.id) set({ installCode: installCode(install.id), installCodeError: '' });
        const server = await addMachine(account, machine, deviceName(), () => { if (sameAccount()) setEntry(machine.machine_id, { phase: 'waiting' }); },
          install.id);
        // O token sai uma vez só e a máquina é desta pessoa: guarda mesmo se a conta mudou.
        const saved = useServers.getState().addConnected(server);
        if (sameAccount()) setEntry(machine.machine_id, null);
        return saved;
      } catch (error) {
        if (!sameAccount()) return null;
        setEntry(machine.machine_id, { phase: 'failed', error: failure(error, 'machine') });
        // O 401 pode ser do bilhete, na máquina, ou da conta, no serviço: reler a lista diz qual foi.
        if (statusOf(error) === 401) void get().refreshMachines();
        return null;
      }
    },
    async enterAll() {
      // Quem já está pedindo segue no pedido dele (`enter` não repete); entram as outras.
      await Promise.all(missingMachines(get().machines, useServers.getState().servers).map((machine) => get().enter(machine)));
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
    refreshDevices() {
      // Pedido durante uma leitura pode vir depois de uma aprovação: a leitura em curso é antiga, então relê ao fim.
      if (devicesRead) { readAgain = true; return devicesRead; }
      const readAll = () => Promise.all(connectServers(useServers.getState().servers).map(async (server) => {
        try {
          const rows = await readDevices(server, AbortSignal.timeout(DEVICES_TIMEOUT_MS));
          set((state) => ({ devices: { ...state.devices, [server.id]: { rows, error: '' } } }));
        } catch (error) {
          set((state) => ({ devices: { ...state.devices, [server.id]: { rows: state.devices[server.id]?.rows ?? null, error: reason(error) } } }));
        }
      }));
      devicesRead = (async () => {
        try {
          do { readAgain = false; await readAll(); } while (readAgain);
        } finally {
          devicesRead = null;
        }
      })();
      return devicesRead;
    },
    async answerDevice(server, deviceId, approve, signal) {
      await (approve ? approveDevice : removeDevice)(server, deviceId, signal);
      await get().refreshDevices();
    },
    async answerGroup(targets, approve) {
      const all = useServers.getState().servers;
      const answered = await Promise.allSettled(targets.map(async ({ serverId, deviceId }) => {
        const server = all.find((candidate) => candidate.id === serverId);
        if (!server) throw new Error(m.connect_group_machine_gone());
        await (approve ? approveDevice : removeDevice)(server, deviceId, AbortSignal.timeout(DEVICES_TIMEOUT_MS));
      }));
      await get().refreshDevices();
      return answered.flatMap((result, i) => result.status === 'rejected' ? [{ label: targets[i].label, error: reason(result.reason) }] : []);
    },
  };
});
