// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AntigravityTermos } from './AntigravityTermos';
import * as m from '../../paraglide/messages';

const calls = vi.hoisted(() => ({
  start: vi.fn(), read: vi.fn(), answer: vi.fn(), cancel: vi.fn(), open: vi.fn(), toast: vi.fn(),
}));
const server = { id: 'b', label: 'B', baseUrl: 'https://b.local', token: 'test' };
const terms = 'https://policies.google.com/terms';
const showing = { attempt_id: 't1', estado: 'termos' as const, texto: `Leia os Termos em ${terms}. Depois escolha.` };

vi.mock('expo-linking', () => ({ openURL: calls.open }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children, open }: { children: ReactNode; open?: boolean }) => open === false ? null : createElement('div', null, children) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/haptics', () => ({ haptic: { selection: vi.fn(), success: vi.fn(), error: vi.fn() } }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: calls.toast, erro: vi.fn() } }));
vi.mock('react-native', async original => ({ ...await original<typeof import('react-native')>(), Alert: { alert: vi.fn() } }));
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  startAntigravityTermsForServer: calls.start,
  getAntigravityTermsForServer: calls.read,
  answerAntigravityTermsForServer: calls.answer,
  cancelAntigravityTermsForServer: calls.cancel,
}));

beforeEach(() => {
  for (const call of Object.values(calls)) call.mockReset();
  calls.start.mockResolvedValue({ attempt_id: 't1', estado: 'iniciando' });
  calls.read.mockResolvedValue(showing);
  calls.answer.mockResolvedValue({ ok: true });
  calls.cancel.mockResolvedValue({ ok: true });
  calls.open.mockResolvedValue(true);
});
afterEach(() => { vi.useRealTimers(); });

const find = (container: HTMLElement, label: string) =>
  [...container.querySelectorAll<HTMLElement>('[aria-label]')].find((node) => node.getAttribute('aria-label') === label)
    ?? [...container.querySelectorAll<HTMLElement>('*')].reverse().find((node) => node.textContent === label);
const press = (container: HTMLElement, label: string) => act(async () => {
  const target = find(container, label);
  expect(target, label).toBeTruthy();
  target!.click();
});
const flush = async () => { for (let i = 0; i < 4; i++) await act(async () => Promise.resolve()); };

function mount() {
  const container = document.createElement('div');
  const root = createRoot(container);
  const props = { server, onDone: vi.fn(), onSignIn: vi.fn(), onOpenTerminal: vi.fn(), onClose: vi.fn() };
  return { container, root, ...props, render: () => act(async () => root.render(createElement(AntigravityTermos, props))) };
}

describe('AntigravityTermos', () => {
  it('sonda a cada 1 s até os Termos; o link abre; a opção tocada vai ao servidor e o aceite fecha com o aviso', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    calls.read.mockResolvedValueOnce({ attempt_id: 't1', estado: 'iniciando' });
    const view = mount();
    try {
      await view.render();
      await flush();
      expect(calls.start).toHaveBeenCalledExactlyOnceWith(server, 'default');
      expect(calls.read).toHaveBeenCalledWith(server, 'default', 't1', expect.any(AbortSignal));
      expect(view.container.textContent).toContain(m.mobile_antigravity_terms_starting());
      expect(find(view.container, m.mobile_antigravity_terms_accept_share())).toBeFalsy();

      await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
      await flush();
      expect(view.container.textContent).toContain('Depois escolha.');
      await press(view.container, terms);
      expect(calls.open).toHaveBeenCalledWith(terms);

      calls.read.mockResolvedValue({ attempt_id: 't1', estado: 'concluido' });
      await press(view.container, m.mobile_antigravity_terms_accept_share());
      await flush();
      expect(calls.answer).toHaveBeenCalledExactlyOnceWith(server, 'default', 't1', 1);
      expect(calls.toast).toHaveBeenCalledWith(m.mobile_antigravity_terms_done());
      expect(view.onDone).toHaveBeenCalledTimes(1);
    } finally { await act(async () => view.root.unmount()); }
    // Concluída, a tentativa já fechou o terminal no servidor.
    expect(calls.cancel).not.toHaveBeenCalled();
  });

  it('aceitar sem compartilhar manda a opção 2 e mostra a espera até o agy confirmar', async () => {
    const view = mount();
    try {
      await view.render();
      await flush();
      calls.read.mockResolvedValue({ attempt_id: 't1', estado: 'respondendo' });
      await press(view.container, m.mobile_antigravity_terms_accept_private());
      await flush();
      expect(calls.answer).toHaveBeenCalledExactlyOnceWith(server, 'default', 't1', 2);
      expect(view.container.textContent).toContain(m.mobile_antigravity_terms_answering());
      expect(find(view.container, m.mobile_antigravity_terms_accept_share())).toBeFalsy();
    } finally { await act(async () => view.root.unmount()); }
  });

  it('Cancelar encerra a tentativa no servidor e volta; sair da tela também encerra', async () => {
    const view = mount();
    await view.render();
    await flush();
    await press(view.container, m.mobile_antigravity_terms_cancel());
    await flush();
    expect(calls.cancel).toHaveBeenCalledExactlyOnceWith(server, 'default', 't1');
    expect(view.onClose).toHaveBeenCalledTimes(1);
    await act(async () => view.root.unmount());

    calls.cancel.mockClear();
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const other = mount();
    await other.render();
    await flush();
    const reads = calls.read.mock.calls.length;
    await act(async () => other.root.unmount());
    expect(calls.cancel).toHaveBeenCalledExactlyOnceWith(server, 'default', 't1');
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(calls.read.mock.calls.length).toBe(reads);
  });

  it('Cancelar com a janela aberta mostra o erro e o Cancelar, sem reler nem trocar para tentativa perdida; o segundo Cancelar manda outro DELETE', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    calls.cancel.mockRejectedValueOnce(Object.assign(new Error('a janela escondida dos Termos não fechou'), { status: 500, code: 'erro_termos_janela_aberta' }));
    const view = mount();
    try {
      await view.render();
      await flush();
      // O servidor já esqueceu a tentativa: uma releitura daria 404.
      calls.read.mockRejectedValue(Object.assign(new Error('404'), { status: 404, code: 'erro_tentativa_inexistente' }));
      const reads = calls.read.mock.calls.length;
      await press(view.container, m.mobile_antigravity_terms_cancel());
      await flush();
      await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
      await flush();
      expect(view.container.textContent).toContain(m.mobile_antigravity_terms_error_window_open());
      expect(view.container.textContent).not.toContain(m.mobile_antigravity_terms_attempt_lost());
      expect(calls.read.mock.calls.length).toBe(reads);
      expect(view.onClose).not.toHaveBeenCalled();

      await press(view.container, m.mobile_antigravity_terms_cancel());
      await flush();
      expect(calls.cancel).toHaveBeenCalledTimes(2);
      expect(calls.cancel).toHaveBeenLastCalledWith(server, 'default', 't1');
      expect(view.onClose).toHaveBeenCalledTimes(1);
    } finally { await act(async () => view.root.unmount()); }
    expect(calls.cancel).toHaveBeenCalledTimes(2);
  });

  it('o prazo total de 10 min tem a sua frase', async () => {
    calls.read.mockResolvedValue({ attempt_id: 't1', estado: 'falhou', codigo: 'termos_expirou' });
    const view = mount();
    try {
      await view.render();
      await flush();
      expect(view.container.textContent).toContain(m.mobile_antigravity_terms_error_expired());
    } finally { await act(async () => view.root.unmount()); }
  });

  it('o agy pedindo login: o texto e Entrar, que leva ao login', async () => {
    calls.read.mockResolvedValue({ attempt_id: 't1', estado: 'login' });
    const view = mount();
    try {
      await view.render();
      await flush();
      expect(view.container.textContent).toContain(m.mobile_antigravity_terms_login());
      await press(view.container, m.contas_entrar());
      expect(view.onSignIn).toHaveBeenCalledTimes(1);
    } finally { await act(async () => view.root.unmount()); }
  });

  it('falha pelo código do servidor: a frase dele e Abrir no terminal; tentativa perdida também', async () => {
    calls.read.mockResolvedValue({ attempt_id: 't1', estado: 'falhou', codigo: 'termos_prazo' });
    const view = mount();
    try {
      await view.render();
      await flush();
      expect(view.container.textContent).toContain(m.mobile_antigravity_terms_error_deadline());
      await press(view.container, m.mobile_antigravity_terms_open_terminal());
      expect(view.onOpenTerminal).toHaveBeenCalledTimes(1);
    } finally { await act(async () => view.root.unmount()); }
    // A tentativa acabou no servidor: nada a encerrar.
    expect(calls.cancel).not.toHaveBeenCalled();

    calls.read.mockRejectedValue(Object.assign(new Error('404'), { status: 404, code: 'erro_tentativa_inexistente' }));
    const lost = mount();
    try {
      await lost.render();
      await flush();
      expect(lost.container.textContent).toContain(m.mobile_antigravity_terms_attempt_lost());
      expect(find(lost.container, m.mobile_antigravity_terms_open_terminal())).toBeTruthy();
    } finally { await act(async () => lost.root.unmount()); }
  });
});
