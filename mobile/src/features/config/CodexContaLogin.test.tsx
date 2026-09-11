// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { CodexContaLogin } from './CodexContaLogin';

const calls = vi.hoisted(() => ({
  create: vi.fn(), prepare: vi.fn(), preparation: vi.fn(), start: vi.fn(), read: vi.fn(), cancel: vi.fn(), accounts: vi.fn(),
  open: vi.fn(), copy: vi.fn(), foreground: undefined as undefined | ((state: string) => void),
}));
const server = { id: 'b', label: 'B', baseUrl: 'https://b.local', token: 'test' };
const attempt = { account_id: 'work', attempt_id: 'try1', status: 'waiting' as const, user_code: 'CODE', verification_url: 'https://auth.example/device' };
const ready = { status: 'ready' as const, issues: [], trust_pending: false };

vi.mock('expo-linking', () => ({ openURL: calls.open }));
vi.mock('expo-clipboard', () => ({ setStringAsync: calls.copy }));
vi.mock('react-native', async original => {
  const actual = await original<typeof import('react-native')>();
  return {
    ...actual,
    AppState: { addEventListener: (_: string, callback: typeof calls.foreground) => { calls.foreground = callback; return { remove: vi.fn() }; } },
    TextInput: (props: { value?: string; onChangeText?: (value: string) => void }) => createElement('textarea', {
      value: props.value,
      onInput: (event: { currentTarget: { value: string } }) => props.onChangeText?.(event.currentTarget.value),
    }),
  };
});
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  createCodexAccountForServer: calls.create,
  prepareCodexAccountForServer: calls.prepare,
  getCodexPreparationForServer: calls.preparation,
  startCodexAccountLoginForServer: calls.start,
  getCodexAccountLoginForServer: calls.read,
  cancelCodexAccountLoginForServer: calls.cancel,
  getCodexAccountsForServer: calls.accounts,
}));

beforeEach(() => {
  for (const call of [calls.create, calls.prepare, calls.preparation, calls.start, calls.read, calls.cancel, calls.accounts,
    calls.open, calls.copy]) call.mockReset();
  calls.foreground = undefined;
  calls.accounts.mockResolvedValue([]);
  calls.prepare.mockResolvedValue(ready);
  calls.preparation.mockResolvedValue(ready);
  calls.read.mockResolvedValue(null);
  calls.open.mockResolvedValue(true);
  calls.copy.mockResolvedValue(true);
  calls.cancel.mockResolvedValue({ ...attempt, status: 'cancelled' });
});

function click(container: HTMLElement, text: string) {
  return act(async () => {
    [...container.querySelectorAll('button')]
      .find((button) => button.textContent === text || button.getAttribute('aria-label') === text
        || (text === 'Entrar' && button.getAttribute('aria-label') === 'Sign in')
        || (text === 'Copiar código' && button.getAttribute('aria-label') === 'Copy code')
        || (text === 'Abrir link' && button.getAttribute('aria-label') === '↗ Open link in browser')
        || (text === 'Cancelar login' && button.getAttribute('aria-label') === 'Cancel sign-in')
        || (text === 'Reconciliar agora' && button.getAttribute('aria-label') === 'Reconcile now'))!
      .click();
  });
}

function render(accountId?: string) {
  const container = document.createElement('div');
  const root = createRoot(container);
  return { container, root, accountId };
}

describe('CodexContaLogin', () => {
  it('reconcilia manualmente a conta escolhida sem iniciar outro login', async () => {
    const { container, root } = render('work');
    try {
      await act(async () => root.render(createElement(CodexContaLogin, { server, accountId: 'work', onComplete: vi.fn() })));
      await click(container, 'Reconciliar agora');
      expect(calls.prepare).toHaveBeenCalledWith(server, 'work', true);
      expect(calls.start).not.toHaveBeenCalled();
    } finally { await act(async () => root.unmount()); }
  });

  it('mantém servidor/conta/tentativa ao copiar, voltar do navegador e cancelar', async () => {
    const { container, root } = render('work');
    calls.start.mockResolvedValue(attempt);
    try {
      await act(async () => root.render(createElement(CodexContaLogin, { server, accountId: 'work', onComplete: vi.fn() })));
      await click(container, 'Entrar');
      expect(calls.start).toHaveBeenCalledWith(server, 'work');
      await click(container, 'Copiar código');
      expect(calls.copy).toHaveBeenCalledWith('CODE');
      await click(container, 'Abrir link');
      expect(calls.open).toHaveBeenCalledWith('https://auth.example/device');
      calls.open.mockRejectedValue(new Error('browser blocked'));
      await click(container, 'Abrir link');
      expect(container.textContent).toContain('browser blocked');
      expect(container.textContent).toContain('CODE');
      calls.read.mockResolvedValue(attempt);
      await act(async () => calls.foreground?.('active'));
      expect(calls.read).toHaveBeenLastCalledWith(server, 'work', expect.any(AbortSignal));
      await click(container, 'Cancelar login');
      expect(calls.cancel).toHaveBeenCalledWith(server, 'work', 'try1');
    } finally { await act(async () => root.unmount()); }
  });

  it('mostra falha ao copiar e mantém o código disponível', async () => {
    const { container, root } = render('work');
    calls.start.mockResolvedValue(attempt);
    calls.copy.mockRejectedValue(new Error('copy failed'));
    try {
      await act(async () => root.render(createElement(CodexContaLogin, { server, accountId: 'work', onComplete: vi.fn() })));
      await click(container, 'Entrar');
      await click(container, 'Copiar código');
      expect(container.textContent).toContain('copy failed');
      expect(container.textContent).toContain('CODE');
    } finally { await act(async () => root.unmount()); }
  });

  it('mostra falha ao cancelar sem perder a tentativa', async () => {
    const { container, root } = render('work');
    calls.read.mockResolvedValue(attempt);
    calls.cancel.mockRejectedValue(new Error('cancel failed'));
    try {
      await act(async () => root.render(createElement(CodexContaLogin, { server, accountId: 'work', onComplete: vi.fn() })));
      await act(async () => Promise.resolve());
      await click(container, 'Cancelar login');
      expect(container.textContent).toContain('cancel failed');
      expect(container.textContent).toContain('CODE');
    } finally { await act(async () => root.unmount()); }
  });

  it('não inicia login quando a preparação falha', async () => {
    const { container, root } = render('work');
    calls.prepare.mockRejectedValue(new Error('prepare failed'));
    try {
      await act(async () => root.render(createElement(CodexContaLogin, { server, accountId: 'work', onComplete: vi.fn() })));
      await click(container, 'Entrar');
      expect(calls.start).not.toHaveBeenCalled();
      expect(container.textContent).toContain('prepare failed');
    } finally { await act(async () => root.unmount()); }
  });

  it('sem conta informada e a padrao sem login, entra na padrao em vez de criar outra', async () => {
    const { container, root } = render();
    calls.accounts.mockResolvedValue([{ id: 'default', is_default: true, auth: { method: 'none', status: 'disconnected' } }]);
    calls.start.mockResolvedValue({ ...attempt, account_id: 'default' });
    try {
      await act(async () => root.render(createElement(CodexContaLogin, { server, onComplete: vi.fn() })));
      expect(container.querySelector('textarea')).toBeNull();
      await click(container, 'Entrar');
      expect(calls.create).not.toHaveBeenCalled();
      expect(calls.start).toHaveBeenCalledWith(server, 'default');
    } finally { await act(async () => root.unmount()); }
  });

  it('cadastra a conta antes de preparar e iniciar o login', async () => {
    const { container, root } = render();
    calls.create.mockResolvedValue({ id: 'new', name: 'work' });
    calls.start.mockResolvedValue({ ...attempt, account_id: 'new' });
    try {
      await act(async () => root.render(createElement(CodexContaLogin, { server, onComplete: vi.fn() })));
      const input = container.querySelector('textarea')!;
      await act(async () => {
        input.value = 'work';
        input.dispatchEvent(new Event('input', { bubbles: true }));
      });
      await click(container, 'Entrar');
      expect(calls.create).toHaveBeenCalledWith(server, 'work');
      expect(calls.prepare).toHaveBeenCalledWith(server, 'new');
      expect(calls.start).toHaveBeenCalledWith(server, 'new');
    } finally { await act(async () => root.unmount()); }
  });
});
