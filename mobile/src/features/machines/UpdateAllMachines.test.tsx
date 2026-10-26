// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ updateAll: vi.fn(), notification: vi.fn(), push: vi.fn(), confirm: true, store: new Map<string, string>() }));
// A folha "Atualizar?" é rota: abrir responde na hora com a escolha do teste.
vi.mock('expo-router', async () => {
  const pending = await import('../updates/pending');
  return { useRouter: () => ({ push: (href: unknown) => {
    mocks.push(href);
    if (href === '/update-confirm') pending.currentConfirmRequest()?.resolve(mocks.confirm);
  } }) };
});
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({
  getString: (k: string) => mocks.store.get(k), set: (k: string, v: string) => { mocks.store.set(k, v); }, remove: (k: string) => { mocks.store.delete(k); },
}) }));
vi.mock('../../stores/servers', () => ({ useServers: (select: (state: { activeId: string | null }) => unknown) => select({ activeId: 'b' }) }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(), updateAll: mocks.updateAll }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(), Platform: { OS: 'ios' } }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => <div>{children}</div> }));
vi.mock('expo-haptics', async (original) => ({ ...await original<typeof import('expo-haptics')>(), notificationAsync: mocks.notification }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
import { UpdateAllMachines } from './UpdateAllMachines';
import * as m from '../../paraglide/messages';
import type { Server, UpdateAllResult, UpdateAllState } from '@hangar/core';

const first = { id: 'a', label: 'fixture-first', baseUrl: 'https://first.test', token: 'fixture-token' };
const second = { ...first, id: 'b', label: 'fixture-second', baseUrl: 'https://second.test' };
let root: Root;
let el: HTMLDivElement;
const render = (servers: Server[] = [first, second]) => act(async () => root.render(<UpdateAllMachines servers={servers} />));
async function click(label: string) {
  const button = [...el.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label);
  expect(button, label).toBeTruthy();
  await act(async () => button!.click());
}
beforeEach(() => {
  mocks.updateAll.mockReset(); mocks.notification.mockReset(); mocks.notification.mockResolvedValue(undefined);
  mocks.push.mockReset(); mocks.confirm = true; mocks.store.clear();
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el);
});
afterEach(async () => { await act(async () => root.unmount()); el.remove(); });

describe('atualizar todas as máquinas', () => {
  it('só oferece a ação com duas máquinas próprias, ligadas e permanentes; não monta a folha antes de abrir', async () => {
    await render([first, { ...second, disabled: true }, { ...second, id: 'invite', invite: true }, { ...second, id: 'temporary', ephemeral: true }]);
    expect(el.querySelector('button')).toBeNull();
    await render([first, second]);
    expect(el.querySelector('button')?.getAttribute('aria-label')).toBe(m.update_all_button({ n: '2' }));
    expect(el.textContent).not.toContain(first.label);
    expect(mocks.updateAll).not.toHaveBeenCalled();
  });

  it('abre, acompanha a rodada sem duplicar ao reabrir e só exibe representantes publicados pelo core', async () => {
    let publish!: (id: string, state: UpdateAllState) => void;
    let finish!: (results: UpdateAllResult[]) => void;
    mocks.updateAll.mockImplementation((_servers, onChange) => {
      publish = onChange;
      return new Promise<UpdateAllResult[]>((resolve) => { finish = resolve; });
    });
    const duplicate = { ...first, id: 'duplicate', label: 'fixture-duplicate' };
    await render([first, second, duplicate]);
    await click(m.update_all_button({ n: '3' }));
    expect(el.textContent).toContain(m.update_all_checking());
    expect(mocks.updateAll).toHaveBeenCalledWith([first, second, duplicate], expect.any(Function));
    await act(async () => {
      publish(first.id, { status: 'rodando', state: { fase: 'rodando', texto: 'fixture-progress', passo: 2, total: 9 }, warnings: [] });
      publish(second.id, { status: 'fora_do_ar', warnings: [] });
    });
    expect(el.textContent).toContain(first.label);
    expect(el.textContent).toContain(m.update_all_running({ texto: 'fixture-progress', passo: '2', total: '9' }));
    expect(el.textContent).toContain(m.update_all_offline());
    expect(el.textContent).not.toContain(duplicate.label);
    await click(m.sessao_fechar());
    expect(el.textContent).not.toContain(first.label);
    await click(m.update_all_button({ n: '3' }));
    expect(mocks.updateAll).toHaveBeenCalledTimes(1);
    await act(async () => {
      const state: UpdateAllState = { status: 'atualizado_manual', warnings: ['fixture-warning'] };
      publish(first.id, state);
      finish([{ server: first, state }, { server: second, state: { status: 'fora_do_ar', warnings: [] } }]);
    });
    expect(el.textContent).toContain(m.update_all_updated_manual());
    expect(el.textContent).toContain(m.update_all_warnings());
    expect(el.textContent).toContain('fixture-warning');
    expect(mocks.notification).toHaveBeenCalledWith('error');
  });

  it('sair da tela no meio da rodada não abre outra ao voltar e mostra a que segue', async () => {
    let publish!: (id: string, state: UpdateAllState) => void;
    let finish!: (results: UpdateAllResult[]) => void;
    mocks.updateAll.mockImplementation((_servers, onChange) => {
      publish = onChange;
      return new Promise<UpdateAllResult[]>((resolve) => { finish = resolve; });
    });
    await render();
    await click(m.update_all_button({ n: '2' }));
    await act(async () => root.unmount());
    root = createRoot(el);
    await render();
    await act(async () => publish(first.id, { status: 'esperando', warnings: [] }));
    await click(m.update_all_button({ n: '2' }));
    expect(mocks.updateAll).toHaveBeenCalledTimes(1);
    expect(el.textContent).toContain(first.label);
    expect(el.textContent).toContain(m.update_all_waiting());
    await act(async () => finish([{ server: first, state: { status: 'atualizado', warnings: [] } }]));
    expect(el.textContent).toContain(m.update_all_updated());
  });

  it.each([
    [{ status: 'consultando' }, () => m.update_all_checking()],
    [{ status: 'iniciando' }, () => m.update_all_starting()],
    [{ status: 'esperando', error: 'fixture-transient-network' }, () => m.update_all_waiting()],
    [{ status: 'em_dia' }, () => m.update_all_up_to_date()],
    [{ status: 'travado', reason: 'branch' }, () => m.update_all_held_branch()],
    [{ status: 'travado', reason: 'mudancas' }, () => m.update_all_held_changes()],
    [{ status: 'travado', reason: 'rodando' }, () => m.update_all_held_running()],
    [{ status: 'travado', reason: 'falta' }, () => m.update_all_held_missing()],
    [{ status: 'atualizado' }, () => m.update_all_updated()],
    [{ status: 'falhou', error: 'fixture-failure' }, () => m.update_all_failed({ erro: 'fixture-failure' })],
    [{ status: 'falhou', errorCode: 'silent' }, () => m.update_all_failed({ erro: m.update_all_silent() })],
    [{ status: 'falhou', errorCode: 'timeout' }, () => m.update_all_failed({ erro: m.update_all_timeout() })],
  ] satisfies [Omit<UpdateAllState, 'warnings'>, () => string][])('mostra o estado do core traduzido: %j', async (value, expected) => {
    mocks.updateAll.mockImplementation(async (_servers, onChange) => {
      const state: UpdateAllState = { ...value, warnings: [] };
      onChange(first.id, state);
      return [{ server: first, state }];
    });
    await render(); await click(m.update_all_button({ n: '2' }));
    expect(el.textContent).toContain(expected());
    expect(el.textContent).not.toContain('fixture-transient-network');
  });

  it('confirma pela folha com os avisos do servidor ativo; cancelar não começa a rodada', async () => {
    mocks.confirm = false;
    await render(); await click(m.update_all_button({ n: '2' }));
    expect(mocks.push).toHaveBeenCalledWith('/update-confirm');
    expect(mocks.updateAll).not.toHaveBeenCalled();
    expect(el.querySelector(`[aria-label="${m.sessao_fechar()}"]`)).toBeNull();
  });

  it('servidor ativo atualizado abre "O que chegou para você" ao fechar os resultados', async () => {
    const { savePendingNews, whatsNewHref } = await import('../updates/pending');
    savePendingNews(second.id, { cards: [{ id: 'n1', title: 'fixture-news', kind: 'new', text: '', platforms: 'all', try_it: null, animation: null }], fixes: [] });
    mocks.updateAll.mockImplementation(async () => [{ server: second, state: { status: 'atualizado', warnings: [] } }]);
    await render(); await click(m.update_all_button({ n: '2' }));
    expect(mocks.push).not.toHaveBeenCalledWith(whatsNewHref(second.id));
    await click(m.sessao_fechar());
    await act(async () => { await new Promise((r) => setTimeout(r, 50)); });
    expect(mocks.push).toHaveBeenLastCalledWith(whatsNewHref(second.id));
  });

  it('vibra sucesso no resultado concluído', async () => {
    mocks.updateAll.mockImplementation(async (_servers, onChange) => {
      const state: UpdateAllState = { status: 'atualizado', warnings: [] };
      onChange(first.id, state);
      return [{ server: first, state }];
    });
    await render(); await click(m.update_all_button({ n: '2' }));
    expect(mocks.notification).toHaveBeenCalledWith('success');
  });

  it('mostra erro geral de uma rodada rejeitada e uma lista vazia sem inventar sucesso', async () => {
    mocks.updateAll.mockRejectedValueOnce(new Error('fixture-rejected')).mockResolvedValueOnce([]);
    await render(); await click(m.update_all_button({ n: '2' }));
    expect(el.textContent).toContain(m.update_all_failed({ erro: 'fixture-rejected' }));
    await click(m.sessao_fechar()); await click(m.update_all_button({ n: '2' }));
    expect(el.textContent).toContain(m.maquinas_vazio());
    expect(el.textContent).not.toContain(m.update_all_updated());
  });

  it('com a tela desmontada, a rodada termina sem desenhar nada e ainda avisa o resultado pela vibração', async () => {
    let publish!: (id: string, state: UpdateAllState) => void;
    let finish!: (results: UpdateAllResult[]) => void;
    mocks.updateAll.mockImplementation((_servers, onChange) => {
      publish = onChange;
      return new Promise<UpdateAllResult[]>((resolve) => { finish = resolve; });
    });
    await render(); await click(m.update_all_button({ n: '2' }));
    await act(async () => root.render(null));
    await act(async () => {
      const state: UpdateAllState = { status: 'atualizado', warnings: [] };
      publish(first.id, state); finish([{ server: first, state }]);
    });
    expect(mocks.notification).toHaveBeenCalledWith('success');
    expect(el.textContent).toBe('');
  });
});
