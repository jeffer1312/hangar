// @vitest-environment happy-dom
// Round 1 da 4b: a tela Servidores NÃO chama store.carregar (zero GET /api/config) — o controller
// da tela é o ServidoresSettings; as outras telas seguem carregando o config do alvo.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount } from 'svelte';
import SettingsModal from './SettingsModal.svelte';
import * as api from '../../lib/api';
import * as auth from '../../lib/auth';
import type { Server } from '../../lib/auth';
import type { TelaConfig } from '../../lib/configRoute';

vi.mock('../../lib/api', () => ({
  getConfig: vi.fn(),
  getConfigForServer: vi.fn(),
  patchConfig: vi.fn(),
  patchConfigForServer: vi.fn(),
  getPushSettings: vi.fn(),
  getPushSettingsForServer: vi.fn(),
  setQuietHours: vi.fn(),
  setQuietHoursForServer: vi.fn(),
}));
vi.mock('../../lib/auth', () => ({
  serverColor: () => '#fff',
  listServers: vi.fn(),
  getActiveId: vi.fn(),
  selectServer: vi.fn(),
  renameServer: vi.fn(),
  updateServer: vi.fn(() => true),
  removeServer: vi.fn(),
  addServer: vi.fn(),
  validarPareamento: vi.fn(),
  clearCredentials: vi.fn(),
  onServersChanged: vi.fn(() => () => {}),
}));
vi.mock('../../lib/sessionsStore.svelte', () => ({
  sessionsStore: { refreshServers: vi.fn(), reconnect: vi.fn() },
}));
vi.mock('../../lib/push', () => ({ enablePush: vi.fn(), pushSupported: () => true }));

const apiMock = vi.mocked(api);
const authMock = vi.mocked(auth);
const SRV = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' };

beforeEach(() => { vi.clearAllMocks(); });   // contagens de chamada não vazam entre testes

function montar(tela: TelaConfig, alvo: Server | null = null, identidade = alvo ? `id:${alvo.id}` : 'global') {
  authMock.listServers.mockReturnValue([SRV as never]);
  authMock.getActiveId.mockReturnValue(SRV.id);
  apiMock.getConfig.mockResolvedValue({ campos: {}, somente_leitura: {} } as never);
  apiMock.getConfigForServer.mockResolvedValue({ campos: {}, somente_leitura: {} } as never);
  apiMock.getPushSettings.mockReturnValue(new Promise(() => {}));
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(SettingsModal, {
    target: el,
    props: {
      tela, alvo, identidade, nomeAlvo: null, semServidor: !alvo,
      onIrPara: vi.fn(), onVoltar: vi.fn(), onFechar: vi.fn(),
    },
  });
  return { el, comp: comp as never };
}

describe('SettingsModal — GET config por tela', () => {
  it('tela servidores: zero GET config', async () => {
    const t = montar('servidores');
    await Promise.resolve();
    expect(apiMock.getConfig).not.toHaveBeenCalled();
    expect(apiMock.getConfigForServer).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('outras telas continuam carregando o config do alvo', async () => {
    const t = montar('anexos', SRV as Server);
    await Promise.resolve();
    expect(apiMock.getConfigForServer).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('identidade composta chega ao store: mesmo id com identidade diferente dispara o próprio GET', async () => {
    // O store observa a identidade (JSON id+label+baseUrl+token), não o id. Montar o MESMO alvo.id
    // com identidades distintas (base/token rotacionados) precisa gerar UM GET por identidade — se
    // o efeito dependesse só do id, o segundo carregaria do cache errado ou nem carregaria.
    const t1 = montar('anexos', SRV as Server, 'id:srv-a::v1');
    await Promise.resolve();
    expect(apiMock.getConfigForServer).toHaveBeenCalledTimes(1);
    unmount(t1.comp);
    const t2 = montar('anexos', SRV as Server, 'id:srv-a::v2');
    await Promise.resolve();
    expect(apiMock.getConfigForServer).toHaveBeenCalledTimes(2);
    unmount(t2.comp);
  });
});
