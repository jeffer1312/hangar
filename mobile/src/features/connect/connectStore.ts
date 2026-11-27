import { create } from 'zustand';
import { Platform } from 'react-native';
import Constants from 'expo-constants';
import * as SecureStore from 'expo-secure-store';
import { addMachine, approveAccountDevice, changeAccountPassword, CONNECT_SERVICE_URL, deviceStatus, endAccountSessions, installCode, listAccountDevices,
  listMachines, normalizeGuestUser, registerDevice, registrarDiag, removeAccountMachine, revokeAccountDevice, signIn, signInAnywhere as signInByName, signUp,
  upgradeToDevice } from '@hangar/core';
import type { AccountDevice, AccountDeviceKey, AccountDeviceStatus, AccountSession, ConnectMachine, Server } from '@hangar/core';
import { validInstallField, vouchAccountDevice } from '../machines/api';
import { syncCrypto } from '../login/syncCrypto';
import { ownServers, useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';

export { CONNECT_SERVICE_URL };
const KEY = 'cp_connect_account_v1';
// Fora da chave da conta: sair e entrar de novo reaproveita o aparelho já liberado.
const DEVICE_KEY = 'cp_connect_device_v1';
const DEVICES_TIMEOUT_MS = 15_000;

export type ConnectAccount = AccountSession & { name: string };
export type MachineEntry = { phase: 'asking' | 'waiting' | 'failed'; error?: string };
export type ThisDevice = AccountDeviceKey & { status: AccountDeviceStatus };
export type DeviceList = { rows: AccountDevice[] | null; error: string };
/** A convidada não abre conta aqui: as máquinas do cofre dela voltam para a tela escolher quais entram. */
export type NameSignIn = { kind: 'account' } | { kind: 'guest'; host: string; servers: Server[] };
type Step = 'signin' | 'signup' | 'password' | 'machine';

export interface ConnectState {
  ready: boolean;
  account: ConnectAccount | null;
  machines: ConnectMachine[] | null;
  machinesError: string;
  loadingMachines: boolean;
  /** Pedido de entrada por `machine_id`. Vive aqui, e não na tela: a aprovação leva até 10 minutos e o token sai uma vez só. */
  entries: Record<string, MachineEntry>;
  /** Este aparelho na conta: pendente espera um aparelho ou PC liberado tocar em Liberar. */
  device: ThisDevice | null;
  deviceError: string;
  /** Aparelhos da conta; só aparelho liberado consegue ler. */
  devices: DeviceList;
  notice: string;
  load(): Promise<void>;
  signIn(name: string, password: string): Promise<void>;
  signUp(name: string, password: string): Promise<void>;
  /** Campo único "conta ou usuário": conta abre aqui; convidada volta com as máquinas dela, sem tocar na conta. */
  signInAnywhere(name: string, password: string): Promise<NameSignIn>;
  signOut(): Promise<void>;
  endSessions(): Promise<void>;
  changePassword(oldPassword: string, newPassword: string): Promise<void>;
  refreshMachines(): Promise<void>;
  enter(machine: ConnectMachine): Promise<Server | null>;
  /** Pede entrada nas máquinas online ainda fora deste aparelho (só nas de `ids`, quando vem) e devolve as que entraram. */
  enterAll(ids?: string[]): Promise<Server[]>;
  reenter(server: Server): Promise<void>;
  turnOnConnect(server: Server): Promise<boolean>;
  /** Registra ou relê este aparelho na conta e, liberado, a lista de aparelhos dela. Nunca rejeita: o erro fica no estado. */
  refreshDevices(): Promise<void>;
  /** Liberar ou recusar o pendente; recusar o liberado tira o acesso dele em todas as máquinas. */
  answerDevice(deviceId: string, approve: boolean): Promise<void>;
  /** Este aparelho perdeu o acesso: registra outro pedido de liberação. */
  requestDeviceAgain(): Promise<void>;
  /** Tira o computador da conta e o servidor dele deste app, e relê a lista. Falha rejeita com a frase da tela. */
  removeMachine(machine: ConnectMachine): Promise<void>;
}

const CODES: Record<string, () => string> = {
  connect_device_pending: m.connect_error_device_pending,
  connect_device_revoked: m.connect_error_device_revoked,
  connect_device_required: m.connect_error_device_required,
  connect_vault_failed: m.connect_error_vault,
  connect_device_denied: m.connect_error_denied,
  connect_approval_expired: m.connect_error_expired,
  connect_token_lost: m.connect_error_token_lost,
  connect_secure_random_unavailable: m.connect_error_secure_random,
  connect_response_invalid: m.mobile_machines_invalid_response,
  connect_install_id_unreadable: m.connect_error_install_id,
  // Entrada pelo nome: conta e convidada caem no mesmo erro, para a tela não dizer qual dos dois existe.
  credentials_mismatch: m.login_credentials_mismatch,
  network_failed: m.connect_error_network,
  connect_too_many_attempts: m.connect_error_limit,
  sync_too_many_attempts: m.connect_error_limit,
  sync_unavailable: m.login_guest_unavailable,
  sync_request_failed: m.login_guest_unavailable,
  sync_insecure: m.login_guest_unavailable,
  sync_vault_unreadable: m.mobile_sync_login_vault,
  sync_vault_missing: m.mobile_sync_login_vault,
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
// O mesmo valor em todos os pedidos deste aparelho: o pedido novo troca o pendente anterior, nunca dá acesso.
// Sem `id` (cofre ou falta de gerador seguro, já no diário), o pedido sai sem ele.
type InstallId = { id?: string };
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
async function readInstallId(): Promise<InstallId> {
  let saved: string | null;
  try { saved = await withinVaultTime(SecureStore.getItemAsync(INSTALL_KEY)); }
  catch {
    // Gerar outro aqui deixaria o pedido antigo pendente ao lado do novo: o pedido para com erro e "tentar de novo"
    // relê o mesmo cofre.
    registrarDiag({ evento: 'connect.install_id_unreadable', nivel: 'aviso', codigo: 'connect_install_id_unreadable' });
    throw Object.assign(new Error(m.connect_error_install_id()), { code: 'connect_install_id_unreadable' });
  }
  if (saved && validInstallField(saved)) return { id: saved };
  // Só aqui gerar outro é certo: o servidor recusaria este em todo pedido.
  if (saved) registrarDiag({ evento: 'connect.install_id_invalid', nivel: 'aviso', codigo: 'connect_install_id_invalid' });
  if (!globalThis.crypto?.getRandomValues) {
    registrarDiag({ evento: 'connect.install_id_no_random', nivel: 'aviso', codigo: 'connect_install_id_no_random' });
    return {};
  }
  const id = Array.from(globalThis.crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, '0')).join('');
  try { await withinVaultTime(SecureStore.setItemAsync(INSTALL_KEY, id)); }
  catch {
    registrarDiag({ evento: 'connect.install_id_unsaved', nivel: 'aviso', codigo: 'connect_install_id_unsaved' });
    return {};
  }
  return { id };
}

/** Código que este celular mostra para quem libera conferir; vazio quando o aparelho não tem identificador. */
export async function readInstallCode(): Promise<string> {
  try {
    const { id } = await installId();
    return id ? installCode(id) : '';
  } catch {
    return '';
  }
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

const sameDevice = (a: AccountDevice, b: AccountDevice) =>
  a.device_id === b.device_id && a.name === b.name && a.status === b.status && a.created_at === b.created_at
  && a.approved_at === b.approved_at && a.revoked_at === b.revoked_at && (a.install_code ?? null) === (b.install_code ?? null);

/** Recusada sem senha do dono para onde voltar: só um pedido novo devolve a entrada. */
export const needsEntry = (server: Server) => !!server.disabled && (server.credential === 'device' || !!server.connectOff);

/** Só quem tem credencial de aparelho está numa máquina com Connect. */
export const connectServers = (servers: Server[]) => ownServers(servers).filter((s) => !s.disabled && s.credential === 'device');

const vaultFailed = () => Object.assign(new Error('connect_vault_failed'), { code: 'connect_vault_failed' });

function savedDevice(raw: string | null, account: ConnectAccount): AccountDeviceKey | null {
  let saved: Record<string, unknown> | null;
  // Registro estragado não volta a valer: um pedido novo substitui.
  try { saved = raw ? JSON.parse(raw) as Record<string, unknown> | null : null; }
  catch { return null; }
  // Aparelho de outra conta (ou serviço) não vale aqui: esta conta registra o seu.
  // A caixa do nome não muda a conta: o aparelho guardado como "Ana" vale para "ana".
  if (!saved || typeof saved.name !== 'string' || normalizeGuestUser(saved.name) !== normalizeGuestUser(account.name)
    || saved.serviceUrl !== account.serviceUrl) return null;
  return typeof saved.id === 'string' && typeof saved.secret === 'string' ? { id: saved.id, secret: saved.secret } : null;
}

export const useConnect = create<ConnectState>((set, get) => {
  let devicesRead: Promise<void> | null = null;
  let readAgain = false;
  let listAnswered = false;
  let machinesRead = 0;
  let newestMachines: Promise<void> = Promise.resolve();
  // `requestDeviceAgain` pula o cofre: o guardado é o revogado.
  let renew = false;
  // As máquinas são perguntadas uma vez por aparelho e login: sem rota (máquina antiga), perguntar de novo não muda nada.
  const vouched = new Set<string>();

  const sessionOf = (account: ConnectAccount): AccountSession => {
    const device = get().device;
    return { ...account, device: device ? { id: device.id, secret: device.secret } : undefined };
  };

  // Pendente que já entrou numa máquina pela aprovação antiga: ela prova o segredo ao serviço e o libera (migração).
  async function vouch(device: AccountDeviceKey): Promise<boolean> {
    const servers = connectServers(useServers.getState().servers);
    if (!servers.length || vouched.has(device.id)) return false;
    vouched.add(device.id);
    for (const server of servers) {
      try {
        await vouchAccountDevice(server, device.id, device.secret, AbortSignal.timeout(DEVICES_TIMEOUT_MS));
        return true;
      } catch (error) {
        registrarDiag({ evento: 'connect.vouch_failed', nivel: 'aviso', codigo: String(statusOf(error) ?? 'connect_vouch_network') }, server.baseUrl);
      }
    }
    return false;
  }

  async function checkDevice(account: ConnectAccount) {
    const before = get().device;
    let key: AccountDeviceKey | null = before;
    if (!key && !renew) {
      try { key = savedDevice(await withinVaultTime(SecureStore.getItemAsync(DEVICE_KEY)), account); }
      // Registrar outro por cima apagaria um aparelho talvez já liberado.
      catch { throw vaultFailed(); }
    }
    let status: AccountDeviceStatus | null = null;
    if (key) {
      // 404: o pedido venceu sem resposta (ou a conta não o conhece): vale um pedido novo.
      try { status = await deviceStatus({ ...account, device: key }); }
      catch (error) { if (statusOf(error) !== 404) throw error; }
    }
    if (!key || !status) {
      key = await registerDevice(account, deviceName(), (await installId()).id);
      try { await withinVaultTime(SecureStore.setItemAsync(DEVICE_KEY, JSON.stringify({ name: account.name, serviceUrl: account.serviceUrl, ...key }))); }
      catch { throw vaultFailed(); }
      renew = false;
      status = 'pending';
    }
    if (status === 'pending' && await vouch(key)) status = await deviceStatus({ ...account, device: key });
    if (get().account?.token !== account.token) return;
    set({ device: { ...key, status }, deviceError: '' });
    // Liberado agora: relê as máquinas para a pessoa escolher em quais entra.
    if (status === 'approved' && before?.status !== 'approved') void get().refreshMachines();
  }

  async function readDevicesOnce() {
    const { account } = get();
    if (!account) return;
    const same = () => get().account?.token === account.token;
    // Liberado com a lista respondendo: a recusa dela já marca pendente ou revogado (`refusedDevice`), e o status à
    // parte seria uma chamada a mais por releitura. Falha da lista volta a conferir o status.
    const known = listAnswered && get().device?.status === 'approved' && !get().deviceError;
    if (!known) {
      listAnswered = false;
      try { await checkDevice(account); }
      catch (error) {
        if (!same()) return;
        if (statusOf(error) === 401) await dropAccount(m.connect_session_ended());
        else set({ deviceError: failure(error, 'machine') });
        return;
      }
    }
    if (get().device?.status !== 'approved') return;
    try {
      const rows = await listAccountDevices(sessionOf(account));
      if (!same()) return;
      listAnswered = true;
      // Releitura igual à anterior mantém o objeto: a lista na tela não redesenha.
      set((state) => {
        const before = state.devices.rows;
        const next = rows.map((row) => before?.find((old) => sameDevice(old, row)) ?? row);
        const unchanged = !state.devices.error && before !== null && before.length === next.length
          && next.every((row, i) => row === before[i]);
        return unchanged ? state : { devices: { rows: next, error: '' } };
      });
    } catch (error) {
      listAnswered = false;
      if (!same()) return;
      if (statusOf(error) === 401) await dropAccount(m.connect_session_ended());
      else { refusedDevice(error); set((state) => ({ devices: { rows: state.devices.rows, error: failure(error, 'machine') } })); }
    }
  }

  // O serviço diz que este aparelho não está liberado: a tela volta a mostrar a espera ou a perda do acesso.
  function refusedDevice(error: unknown) {
    const code = (error as { code?: string }).code;
    const status = code === 'connect_device_revoked' ? 'revoked' : code === 'connect_device_pending' ? 'pending' : null;
    if (status) set((state) => (state.device ? { device: { ...state.device, status } } : {}));
  }

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

  const noDevices: DeviceList = { rows: null, error: '' };
  const signedOut = (notice: string) => set({ account: null, machines: null, machinesError: '', loadingMachines: false, entries: {},
    device: null, deviceError: '', devices: noDevices, notice });

  // O token já não vale: a falha ao apagar só deixa um token morto, que o próximo 401 limpa.
  async function dropAccount(notice: string) {
    await SecureStore.deleteItemAsync(KEY).catch(() => {
      registrarDiag({ evento: 'connect.conta_nao_apagada', nivel: 'aviso', codigo: 'connect_account_delete_failed' });
    });
    signedOut(notice);
  }

  async function openAccount(login: typeof signIn, step: Step, rawName: string, password: string) {
    const name = normalizeGuestUser(rawName);
    let session: AccountSession;
    try { session = await login(CONNECT_SERVICE_URL, name, password); }
    catch (error) { throw new Error(failure(error, step)); }
    await adopt(session, name);
  }

  async function adopt(session: AccountSession, name: string) {
    const account = { ...session, name: normalizeGuestUser(name) };
    try { await SecureStore.setItemAsync(KEY, JSON.stringify(account)); }
    catch { throw new Error(m.connect_error_vault()); }
    vouched.clear();
    set({ account, machines: null, machinesError: '', entries: {}, device: null, deviceError: '', devices: noDevices, notice: '' });
    void get().refreshMachines();
    // No login: o aparelho se registra (ou reaproveita o guardado) e diz se já está liberado.
    await get().refreshDevices();
  }

  return {
    ready: false,
    account: null,
    machines: null,
    machinesError: '',
    loadingMachines: false,
    entries: {},
    device: null,
    deviceError: '',
    devices: noDevices,
    notice: '',
    async load() {
      if (get().ready) return;
      try {
        const raw = await SecureStore.getItemAsync(KEY);
        const saved: unknown = raw === null ? null : JSON.parse(raw);
        if (saved !== null && !isAccount(saved)) throw new Error('connect_account_unreadable');
        set({ ready: true, account: isAccount(saved) ? { ...saved, name: normalizeGuestUser(saved.name) } : null });
      } catch {
        registrarDiag({ evento: 'connect.conta_ilegivel', nivel: 'aviso', codigo: 'connect_account_unreadable' });
        set({ ready: true, notice: m.connect_error_vault() });
      }
    },
    signIn: (name, password) => openAccount(signIn, 'signin', name, password),
    signUp: (name, password) => openAccount(signUp, 'signup', name, password),
    async signInAnywhere(rawName, password) {
      const name = normalizeGuestUser(rawName);
      let found: Awaited<ReturnType<typeof signInByName>>;
      try { found = await signInByName(name, password, { crypto: syncCrypto, serviceUrl: CONNECT_SERVICE_URL }); }
      catch (error) { throw new Error(failure(error, 'signin')); }
      if (found.kind === 'guest') return { kind: 'guest', host: found.host, servers: found.servers };
      await adopt(found.account, name);
      return { kind: 'account' };
    },
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
        const server = await addMachine(sessionOf(account), machine, deviceName(),
          () => { if (sameAccount()) setEntry(machine.machine_id, { phase: 'waiting' }); }, install.id);
        // O token sai uma vez só e a máquina é desta pessoa: guarda mesmo se a conta mudou.
        const saved = useServers.getState().addConnected(server);
        if (sameAccount()) setEntry(machine.machine_id, null);
        return saved;
      } catch (error) {
        if (!sameAccount()) return null;
        refusedDevice(error);
        setEntry(machine.machine_id, { phase: 'failed', error: failure(error, 'machine') });
        // O 401 pode ser do bilhete, na máquina, ou da conta, no serviço: reler a lista diz qual foi.
        if (statusOf(error) === 401) void get().refreshMachines();
        return null;
      }
    },
    async enterAll(ids) {
      // Quem já está pedindo segue no pedido dele (`enter` não repete); entram as outras.
      const chosen = missingMachines(get().machines, useServers.getState().servers)
        .filter((machine) => !ids || ids.includes(machine.machine_id));
      const saved = await Promise.all(chosen.map((machine) => get().enter(machine)));
      return saved.filter((server): server is Server => !!server);
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
      // Uma leitura por vez também impede dois registros do mesmo aparelho.
      if (devicesRead) { readAgain = true; return devicesRead; }
      devicesRead = (async () => {
        try {
          do { readAgain = false; await readDevicesOnce(); } while (readAgain);
        } finally {
          devicesRead = null;
        }
      })();
      return devicesRead;
    },
    async answerDevice(deviceId, approve) {
      const { account } = get();
      if (!account) throw new Error(m.connect_session_ended());
      try { await (approve ? approveAccountDevice : revokeAccountDevice)(sessionOf(account), deviceId); }
      catch (error) { throw new Error(failure(error, 'machine')); }
      await get().refreshDevices();
    },
    async requestDeviceAgain() {
      renew = true;
      set({ device: null, deviceError: '', devices: noDevices });
      await get().refreshDevices();
    },
    async removeMachine(machine) {
      const { account } = get();
      if (!account) throw new Error(m.connect_session_ended());
      try { await removeAccountMachine(sessionOf(account), machine.machine_id); }
      catch (error) {
        // 404: outro aparelho já o tirou da conta; falta só a limpeza daqui.
        if (statusOf(error) === 401) {
          if (get().account?.token === account.token) await dropAccount(m.connect_session_ended());
          throw new Error(m.connect_session_ended());
        }
        if (statusOf(error) !== 404) throw new Error(failure(error, 'machine'));
      }
      // Fora da conta ele deixa de abrir pelo Connect: a entrada guardada só ficaria recusando.
      const servers = useServers.getState();
      for (const server of ownServers(servers.servers).filter((candidate) => sameMachine(candidate, machine))) servers.remove(server.id);
      setEntry(machine.machine_id, null);
      if (get().account?.token === account.token) await get().refreshMachines();
    },
  };
});
