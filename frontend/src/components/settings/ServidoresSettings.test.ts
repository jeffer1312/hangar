// @vitest-environment happy-dom
// Round 1/2 da 4b: logout idempotente — Sair e remover-último chamam onLogout UMA vez; enquanto a
// Promise anda, as portas de saída ficam bloqueadas; rejeição vira erro visível recuperável (o
// clear de credenciais é dono do App/lib/logout.ts, este componente não chama).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ServidoresSettings from './ServidoresSettings.svelte';
import * as m from '../../paraglide/messages';
import * as auth from '../../lib/auth';
import * as api from '../../lib/api';
import type { Server } from '../../lib/auth';

let mudouCb: (() => void) | null = null;
// importOriginal mantém REAIS os helpers de remoção (serverFingerprint/snapshotRemocao/
// removalStillMatches) — o componente precisa deles funcionando pra revisar a entidade; o resto
// (mutadores/leitores de localStorage) fica mockado.
vi.mock('../../lib/auth', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/auth')>();
  return {
    ...real,
    serverColor: () => '#fff',
    listServers: vi.fn(),
    getActiveId: vi.fn(),
    selectServer: vi.fn(),
    renameServer: vi.fn(),
    updateServer: vi.fn(() => true),
    removeServer: vi.fn(),
    addServer: vi.fn(),
    addServerWithRollback: vi.fn(async () => ({ id: 'srv-x', succeeded: true })),
    validarPareamento: vi.fn(),
    onServersChanged: vi.fn((cb: () => void) => { mudouCb = cb; return () => {}; }),
  };
});
vi.mock('../../lib/sessionsStore.svelte', () => ({
  sessionsStore: { refreshServers: vi.fn(), reconnect: vi.fn() },
}));
vi.mock('../../lib/api', () => ({
  getSessions: vi.fn(async () => []),
  getPushSettings: vi.fn(),
  getPushSettingsForServer: vi.fn(),
  setQuietHours: vi.fn(),
  setQuietHoursForServer: vi.fn(),
}));
vi.mock('../../lib/push', () => ({ enablePush: vi.fn(), pushSupported: () => true }));
vi.mock('../../lib/peers', () => ({
  getIdentificador: vi.fn(async () => ({ identificador: '' })),
  setIdentificador: vi.fn(async (v: string) => ({ identificador: v })),
  listarPeers: vi.fn(async () => []),
  gravarPeer: vi.fn(async (d: unknown) => [d]),
  removerPeer: vi.fn(async () => []),
}));

const authMock = vi.mocked(auth);
const apiMock = vi.mocked(api);
const peersMock = vi.mocked(await import('../../lib/peers'));
const SRV: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' } as Server;

let onLogoutResolve: (() => void) | null = null;
function onLogoutDeferred() {
  return new Promise<void>((res) => { onLogoutResolve = res; });
}

let onLogoutCalls: ReturnType<typeof vi.fn<() => Promise<void>>>;

function montar(over: { onLogout?: () => Promise<void> } = {}) {
  authMock.listServers.mockReturnValue([SRV]);
  authMock.getActiveId.mockReturnValue(SRV.id);
  apiMock.getPushSettings.mockReturnValue(new Promise(() => {}));   // fica pendente (irrelevante)
  onLogoutCalls = vi.fn<() => Promise<void>>(over.onLogout ?? onLogoutDeferred);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ServidoresSettings, {
    target: el,
    props: { resolvedServer: SRV, apiTarget: null, onPickTarget: vi.fn(), onLogout: onLogoutCalls },
  });
  return { el, comp: comp as never };
}

beforeEach(() => { vi.clearAllMocks(); onLogoutResolve = null; mudouCb = null; });

describe('ServidoresSettings — logout idempotente', () => {
  it('Sair + remover-último durante a Promise chamam onLogout UMA vez', async () => {
    const t = montar();
    // 1) Sair -> confirmação -> logout começa (Promise pendente). O ModalDialog vive num portal
    // pro <body>, então o diálogo se busca no document, não dentro de t.el.
    t.el.querySelector<HTMLButtonElement>('.ss-danger')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    // 2) Durante a Promise: remover (×) fica bloqueado — diálogo nem abre
    const del = t.el.querySelector<HTMLButtonElement>('.sm-srv-del');
    expect(del).not.toBeNull();   // podeRemoverUltimo: visível
    del!.click();
    await tick();
    expect(document.querySelector('.confirm-card')).toBeNull();   // guarda segurou
    // 3) Resolve: segue 1 chamada só
    onLogoutResolve!();
    await Promise.resolve(); await Promise.resolve();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('segundo disparo durante a Promise é bloqueado (Sair disabled e remover sem diálogo)', async () => {
    const t = montar();
    t.el.querySelector<HTMLButtonElement>('.ss-danger')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    // Durante a Promise: Sair principal disabled (clique é no-op) e remover nem abre diálogo
    const sairDeNovo = t.el.querySelector<HTMLButtonElement>('.ss-danger')!;
    expect(sairDeNovo.disabled).toBe(true);
    sairDeNovo.click();
    t.el.querySelector<HTMLButtonElement>('.sm-srv-del')!.click();
    await tick();
    expect(document.querySelector('.confirm-card')).toBeNull();
    // Resolve: segue 1 chamada só
    onLogoutResolve!();
    await Promise.resolve(); await Promise.resolve();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('remoção revalida fingerprint: servidor mudou no sync → aviso, sem remover', async () => {
    const t = montar();
    t.el.querySelector<HTMLButtonElement>('.sm-srv-del')!.click();
    await tick();
    // o sync alterou o servidor entre o diálogo e o clique (onServersChanged sobe a versão local)
    authMock.listServers.mockReturnValue([{ ...SRV, token: 'novo-token' }]);
    mudouCb?.();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(authMock.removeServer).not.toHaveBeenCalled();
    const aviso = t.el.querySelector<HTMLElement>('.ss-aviso');
    expect(aviso?.innerText).toContain(m.config_servidores_aviso_lista_mudou());
    expect(aviso?.getAttribute('role')).toBe('status');
    unmount(t.comp);
  });

  it('rejeição do onLogout vira erro visível e libera nova tentativa (sem unhandled)', async () => {
    const t = montar({ onLogout: () => Promise.reject(new Error('x')) });
    t.el.querySelector<HTMLButtonElement>('.ss-danger')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await Promise.resolve();   // o catch do logout roda em microtask; só então a mensagem renderiza
    await tick();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    const aviso = t.el.querySelector<HTMLElement>('.ss-aviso');
    expect(aviso?.innerText).toContain(m.config_servidores_sair_erro());
    expect(aviso?.getAttribute('role')).toBe('status');
    // Guard resetado: nova tentativa funciona
    t.el.querySelector<HTMLButtonElement>('.ss-danger')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await Promise.resolve();
    await tick();
    expect(onLogoutCalls).toHaveBeenCalledTimes(2);
    unmount(t.comp);
  });
});

describe('ServidoresSettings — remoção com fingerprint + revision (round 4)', () => {
  async function confirmarRemocao(t: { el: HTMLElement }) {
    t.el.querySelector<HTMLButtonElement>('.sm-srv-del')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
  }

  it('entidade inalterada: removeServer é chamado UMA vez com o id certo', async () => {
    const t = montar();
    authMock.getActiveId.mockReturnValue('outro-id');   // remover SRV não é remover o ativo -> sem reload
    await confirmarRemocao(t);
    expect(authMock.removeServer).toHaveBeenCalledTimes(1);
    expect(authMock.removeServer).toHaveBeenCalledWith(SRV.id);
    unmount(t.comp);
  });

  it('servidor ausente entre diálogo e clique: não remove, aviso role=status', async () => {
    const t = montar();
    t.el.querySelector<HTMLButtonElement>('.sm-srv-del')!.click();
    await tick();
    authMock.listServers.mockReturnValue([]);   // apagado noutro aparelho ANTES do clique
    await tick();                                // revision inalterada (sem mudouCb)
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(authMock.removeServer).not.toHaveBeenCalled();
    const aviso = t.el.querySelector<HTMLElement>('.ss-aviso');
    expect(aviso?.innerText).toContain(m.config_servidores_aviso_ja_removido());
    expect(aviso?.getAttribute('role')).toBe('status');
    unmount(t.comp);
  });

  it('último servidor removido em Settings chama onLogout UMA vez', async () => {
    // removeServer de verdade esvazia a lista -> o controller vê "zerou" e dispara o logout global.
    authMock.removeServer.mockImplementation(() => {
      authMock.listServers.mockReturnValue([]);
    });
    const t = montar();
    await confirmarRemocao(t);
    expect(authMock.removeServer).toHaveBeenCalledTimes(1);
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('botão Add desabilitado durante a transação; uma tentativa por clique; erro visível com retry (round 5)', async () => {
    authMock.validarPareamento.mockReturnValue({ base: 'http://a', token: 'x' });
    let rejectAdd!: (e: Error) => void;
    authMock.addServerWithRollback.mockReturnValueOnce(new Promise((_res, rej) => { rejectAdd = rej; }));
    const t = montar();
    t.el.querySelector<HTMLButtonElement>('.sm-item')!.click();   // "Adicionar servidor"
    await tick(); await tick();
    const addBtn = document.querySelector<HTMLButtonElement>('.confirm-card .c-primary')!;
    const qrBtn = document.querySelector<HTMLButtonElement>('.confirm-card .c-btn:not(.c-primary)')!;
    const input = document.querySelector<HTMLInputElement>('.ss-add-input')!;
    input.value = 'http://a/?token=x';
    input.dispatchEvent(new Event('input'));
    await tick();   // re-render do disabled (bind:value) antes do clique
    addBtn.click();
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(1);
    expect(addBtn.disabled).toBe(true);   // Add bloqueado durante o await
    expect(qrBtn.disabled).toBe(true);    // acionador de QR bloqueado também
    addBtn.click();                       // segundo clique não abre outra tentativa
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(1);
    rejectAdd(new Error('servidor fora do ar'));
    await tick(); await tick();
    expect(addBtn.disabled).toBe(false);  // finally libera o botão
    const err = document.querySelector<HTMLElement>('#ss-add-err');
    expect(err?.innerText).toContain('servidor fora do ar');
    expect(err?.getAttribute('role')).toBe('alert');
    // retry: nova tentativa roda
    authMock.addServerWithRollback.mockReturnValueOnce(new Promise(() => {}));
    addBtn.click();
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(2);
    unmount(t.comp);
  });

  // Round 6: o botão é disabled, mas Enter chama o handler direto — o guard no handler é o que
  // impede a segunda transação; input fica travado durante o await e o retry roda após o finally.
  it('Enter no input durante a transação não inicia segunda tentativa; retry após reject', async () => {
    authMock.validarPareamento.mockReturnValue({ base: 'http://a', token: 'x' });
    let rejectAdd!: (e: Error) => void;
    authMock.addServerWithRollback.mockReturnValueOnce(new Promise((_res, rej) => { rejectAdd = rej; }));
    const t = montar();
    t.el.querySelector<HTMLButtonElement>('.sm-item')!.click();   // "Adicionar servidor"
    await tick(); await tick();
    const input = document.querySelector<HTMLInputElement>('.ss-add-input')!;
    input.value = 'http://a/?token=x';
    input.dispatchEvent(new Event('input'));
    await tick();
    document.querySelector<HTMLButtonElement>('.confirm-card .c-primary')!.click();
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(1);
    expect(input.disabled).toBe(true);   // input travado durante a transação
    // Enter enquanto pendente: ignorado (handler tem guard)
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(1);
    rejectAdd(new Error('servidor fora do ar'));
    await tick(); await tick();
    expect(document.querySelector<HTMLElement>('#ss-add-err')?.innerText).toContain('servidor fora do ar');
    expect(input.disabled).toBe(false);
    // retry via Enter após o finally: segunda tentativa roda
    authMock.addServerWithRollback.mockReturnValueOnce(new Promise(() => {}));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(2);
    unmount(t.comp);
  });

  // Round 7: erro TARDIO — o probe rejeita DEPOIS de o usuário fechar o diálogo de colar URL.
  // O erro não pode sumir calado: o modal reabre com a mensagem visível pra retry (mesmo
  // caminho que o QR já fazia).
  it('fechar o diálogo durante a transação não engole o erro tardio: reabre com role=alert', async () => {
    authMock.validarPareamento.mockReturnValue({ base: 'http://a', token: 'x' });
    let rejectAdd!: (e: Error) => void;
    authMock.addServerWithRollback.mockReturnValueOnce(new Promise((_res, rej) => { rejectAdd = rej; }));
    const t = montar();
    t.el.querySelector<HTMLButtonElement>('.sm-item')!.click();
    await tick(); await tick();
    const input = document.querySelector<HTMLInputElement>('.ss-add-input')!;
    input.value = 'http://a/?token=x';
    input.dispatchEvent(new Event('input'));
    await tick();
    document.querySelector<HTMLButtonElement>('.confirm-card .c-primary')!.click();
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(1);
    // usuário fecha o diálogo (Esc) com a transação ainda pendente
    document.querySelector<HTMLElement>('.modal-dialog')!
      .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await tick();
    expect(document.querySelector('.confirm-card')).toBeNull();
    // o probe falha DEPOIS do fechamento: erro visível, modal reaberto
    rejectAdd(new Error('servidor fora do ar'));
    await tick(); await tick();
    expect(document.querySelector('.confirm-card')).not.toBeNull();
    const err = document.querySelector<HTMLElement>('#ss-add-err');
    expect(err?.innerText).toContain('servidor fora do ar');
    expect(err?.getAttribute('role')).toBe('alert');
    unmount(t.comp);
  });

  it('cancelar a confirmação devolve o foco ao botão Remover (restauração segura)', async () => {
    const t = montar();
    authMock.getActiveId.mockReturnValue('outro-id');   // remover não é remover o ativo -> sem reload
    const del = t.el.querySelector<HTMLButtonElement>('.sm-srv-del')!;
    del.focus();   // happy-dom não move foco no click() — o gatilho precisa de focus explícito
    del.click();
    await tick(); await tick();
    // o foco saiu do gatilho (foi pra dentro do diálogo)
    expect(document.activeElement).not.toBe(del);
    const cancel = document.querySelector<HTMLElement>('.confirm-card')!
      .querySelector<HTMLButtonElement>('.c-btn:not(.c-danger)')!;
    cancel.click();
    await tick();
    // ConfirmDialog desmontou (pendingRemoval=null) e o ModalDialog restaurou o foco pro gatilho
    expect(document.activeElement).toBe(del);
    unmount(t.comp);
  });
});

// ── Seções da Task 5: identificador desta máquina + máquinas que este servidor alcança ───
describe('ServidoresSettings — identificador e peers (Task 5)', () => {
  async function esperarCarga() {
    await Promise.resolve(); await Promise.resolve(); await tick();
  }

  it('estado 1 do mock: sem identificador, aviso visível e seção de alcance vazia, sem registrar', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    peersMock.listarPeers.mockResolvedValue([]);
    const t = montar();
    await esperarCarga();
    expect(peersMock.getIdentificador).toHaveBeenCalledWith(null);  // alvo null = servidor ativo
    // Esta máquina: rótulo + legenda + aviso, campo com borda de aviso e placeholder
    expect(t.el.textContent).toContain(m.peers_esta_maquina());
    expect(t.el.textContent).toContain(m.peers_legenda_identificador());
    const aviso = t.el.querySelector('.id-aviso');
    expect(aviso?.textContent).toContain(m.peers_aviso_nao_definido());
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo');
    expect(campo?.classList.contains('vazio')).toBe(true);
    expect(campo?.value).toBe('');
    expect(campo?.getAttribute('placeholder')).toContain('casa');
    // alcance: só a legenda de vazio, sem cartão nem botão de registrar (mock estado 1)
    expect(t.el.textContent).toContain(m.peers_vazio());
    expect(t.el.querySelector('.pr-cartao')).toBeNull();
    expect(t.el.querySelector('.pr-btn')).toBeNull();
    unmount(t.comp);
  });

  it('estado 2 do mock: identificador definido, peer listado com endereço e Remover', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: '••••reto' },
    ]);
    const t = montar();
    await esperarCarga();
    expect(t.el.textContent).toContain(m.peers_identificador_definido({ nome: 'casa' }));
    expect(t.el.querySelector('.id-aviso')).toBeNull();          // definido: sem aviso
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo');
    expect(campo?.classList.contains('vazio')).toBe(false);
    expect(campo?.value).toBe('casa');
    expect(t.el.textContent).toContain(m.peers_legenda_alcance());
    const nome = t.el.querySelector('.pr-nome');
    expect(nome?.textContent).toBe('notebook');
    expect(t.el.querySelector('.pr-url')?.textContent).toContain('192.168.0.77');
    expect(t.el.querySelector('.pr-btn.primaria')?.textContent).toContain(m.peers_registrar());
    unmount(t.comp);
  });

  it('Enter no campo grava o identificador; inválido mostra a dica e não grava', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    peersMock.setIdentificador.mockResolvedValue({ identificador: 'casa' });
    const t = montar();
    await esperarCarga();
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo')!;
    campo.value = 'casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(peersMock.setIdentificador).toHaveBeenCalledWith(null, 'casa');
    expect(t.el.querySelector('.id-aviso')).toBeNull();          // virou definido
    // inválido (maiúscula): a dica É a regra, e nada vai pro backend
    campo.value = 'Casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(t.el.querySelector('.id-erro')?.textContent).toContain(m.peers_identificador_dica({ exemplos: 'casa, notebook' }));
    expect(peersMock.setIdentificador).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('registrar um peer: diálogo com id/endereço/token grava e a lista atualiza', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.gravarPeer.mockResolvedValue([
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ]);
    const t = montar();
    await esperarCarga();
    t.el.querySelector<HTMLButtonElement>('.pr-btn.primaria')!.click();
    await tick();
    // rótulo curto no botão primário do diálogo: "Confirmar" cabe em uma linha (bloq 3)
    expect(document.querySelector('.confirm-card .c-primary')?.textContent?.trim())
      .toBe(m.comum_confirmar());
    const inputs = document.querySelectorAll<HTMLInputElement>('.pr-form-input');
    inputs[0].value = 'notebook';
    inputs[0].dispatchEvent(new Event('input'));
    inputs[1].value = 'http://192.168.0.77:8765';
    inputs[1].dispatchEvent(new Event('input'));
    inputs[2].value = 'segredo';
    inputs[2].dispatchEvent(new Event('input'));
    await tick();
    document.querySelector<HTMLButtonElement>('.confirm-card .c-primary')!.click();
    await tick(); await tick();
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(null, {
      id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo',
    });
    expect(document.querySelector('.confirm-card')).toBeNull();  // fechou no sucesso
    expect(t.el.querySelector('.pr-nome')?.textContent).toBe('notebook');
    unmount(t.comp);
  });

  it('remover um peer pede confirmação e chama o backend com o id', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([
      { id: 'notebook', base_url: 'http://n:8765', token: '••••' },
    ]);
    const t = montar();
    await esperarCarga();
    t.el.querySelector<HTMLButtonElement>('.pr-btn.min')!.click();
    await tick();
    expect(document.querySelector('.confirm-card')).not.toBeNull();
    document.querySelector<HTMLButtonElement>('.confirm-card .c-danger')!.click();
    await tick();
    expect(peersMock.removerPeer).toHaveBeenCalledWith(null, 'notebook');
    unmount(t.comp);
  });

  it('fala com o servidor ESCOLHIDO na aba, não com o ativo', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([]);
    const outro = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' } as Server;
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ServidoresSettings, { target: el, props: {
      resolvedServer: outro, apiTarget: outro, onPickTarget: vi.fn(), onLogout: vi.fn() } });
    await esperarCarga();
    expect(peersMock.getIdentificador).toHaveBeenCalledWith(outro);
    expect(peersMock.listarPeers).toHaveBeenCalledWith(outro);
    unmount(comp as never);
  });

  it('servidor indisponível não carrega nada', async () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ServidoresSettings, { target: el, props: {
      resolvedServer: null, apiTarget: null, onPickTarget: vi.fn(), onLogout: vi.fn() } });
    await esperarCarga();
    expect(peersMock.getIdentificador).not.toHaveBeenCalled();
    expect(peersMock.listarPeers).not.toHaveBeenCalled();
    unmount(comp as never);
  });

  it('falha ao listar peers aparece com nome, e não some quando não é Error', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockRejectedValueOnce(new Error('Failed to fetch'));
    const t = montar();
    await esperarCarga();
    expect(t.el.querySelector('.id-erro')?.textContent).toContain(m.falha_conexao());
    unmount(t.comp);

    peersMock.listarPeers.mockRejectedValueOnce('caiu');   // rejeição que NÃO é Error
    const t2 = montar();
    await esperarCarga();
    expect(t2.el.querySelector('.id-erro')?.textContent).toContain(m.erro_desconhecido());
    unmount(t2.comp);
  });

  it('peer existente aparece mesmo sem identificador, e a tela não diz "nenhuma"', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    peersMock.listarPeers.mockResolvedValue([
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: '••••reto' },
    ]);
    const t = montar();
    await esperarCarga();
    expect(t.el.querySelector('.pr-nome')?.textContent).toBe('notebook');
    expect(t.el.textContent).not.toContain(m.peers_vazio());
    expect(t.el.querySelector('.pr-btn.primaria')).toBeNull();   // sem identificador, sem registrar
    unmount(t.comp);
  });

  it('identificador definido e lista vazia: legenda explica o registro, não manda definir o id', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([]);
    const t = montar();
    await esperarCarga();
    expect(t.el.textContent).not.toContain(m.peers_vazio());
    expect(t.el.textContent).toContain(m.peers_legenda_alcance());
    expect(t.el.querySelector('.pr-btn.primaria')).not.toBeNull();
    unmount(t.comp);
  });

  it('enquanto a listagem não volta, a tela não afirma que não há nenhum', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockReturnValueOnce(new Promise(() => {}));   // nunca resolve
    const t = montar();
    await esperarCarga();
    expect(t.el.textContent).not.toContain(m.peers_vazio());
    expect(t.el.textContent).toContain(m.comum_carregando());
    unmount(t.comp);
  });

  it('Enter não dispara dois PUT: o blur causado pelo disabled é ignorado', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    let resolver!: (v: { identificador: string }) => void;
    peersMock.setIdentificador.mockReturnValueOnce(new Promise((r) => { resolver = r; }));
    const t = montar();
    await esperarCarga();
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo')!;
    campo.value = 'casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    campo.dispatchEvent(new Event('blur'));      // o blur que o disabled provoca no navegador
    await tick();
    expect(peersMock.setIdentificador).toHaveBeenCalledTimes(1);
    resolver({ identificador: 'casa' });
    await tick(); await tick();
    expect(peersMock.setIdentificador).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('o campo não é desabilitado durante a gravação (o foco não sai dele)', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    let resolver!: (v: { identificador: string }) => void;
    peersMock.setIdentificador.mockReturnValueOnce(new Promise((r) => { resolver = r; }));
    const t = montar();
    await esperarCarga();
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo')!;
    campo.value = 'casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(campo.disabled).toBe(false);     // disabled tiraria o foco do campo
    expect(campo.readOnly).toBe(true);      // e readonly impede a edição sem tirar
    resolver({ identificador: 'casa' });
    await tick(); await tick();
    expect(campo.readOnly).toBe(false);
    unmount(t.comp);
  });
});
