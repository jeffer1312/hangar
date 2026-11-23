// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AntigravityContaLogin } from './AntigravityContaLogin';
import * as m from '../../paraglide/messages';

const calls = vi.hoisted(() => ({
  create: vi.fn(), start: vi.fn(), read: vi.fn(), submit: vi.fn(), cancel: vi.fn(),
  open: vi.fn(), copy: vi.fn(), paste: vi.fn(), foreground: undefined as undefined | ((state: string) => void),
}));
const server = { id: 'b', label: 'B', baseUrl: 'https://b.local', token: 'test' };
const link = 'https://accounts.google.com/o/oauth2/auth?redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback';
const waiting = { attempt_id: 'a1', estado: 'aguardando_codigo' as const, link };
const accounts = [
  { id: 'default', padrao: true, pasta: '/h/.gemini', credencial: 'antigravity:/h/.gemini', logado: true, termos_aceitos: true, em_uso: false },
];

vi.mock('expo-linking', () => ({ openURL: calls.open }));
vi.mock('expo-clipboard', () => ({ setStringAsync: calls.copy, getStringAsync: calls.paste }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children, open }: { children: ReactNode; open?: boolean }) => open === false ? null : createElement('div', null, children) }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/haptics', () => ({ haptic: { selection: vi.fn(), success: vi.fn(), error: vi.fn() } }));
vi.mock('react-native', async original => {
  const actual = await original<typeof import('react-native')>();
  return {
    ...actual,
    Alert: { alert: vi.fn() },
    AppState: { addEventListener: (_: string, callback: typeof calls.foreground) => { calls.foreground = callback; return { remove: vi.fn() }; } },
    TextInput: (props: { value?: string; accessibilityLabel?: string; onChangeText?: (value: string) => void }) => createElement('textarea', {
      value: props.value,
      'aria-label': props.accessibilityLabel,
      onInput: (event: { currentTarget: { value: string } }) => props.onChangeText?.(event.currentTarget.value),
    }),
  };
});
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  createAntigravityAccountForServer: calls.create,
  startAntigravityLoginForServer: calls.start,
  getAntigravityLoginForServer: calls.read,
  submitAntigravityLoginCodeForServer: calls.submit,
  cancelAntigravityLoginForServer: calls.cancel,
}));

beforeEach(() => {
  for (const call of [calls.create, calls.start, calls.read, calls.submit, calls.cancel, calls.open, calls.copy, calls.paste]) call.mockReset();
  calls.foreground = undefined;
  calls.start.mockResolvedValue({ attempt_id: 'a1', estado: 'iniciando' });
  calls.read.mockResolvedValue(waiting);
  calls.submit.mockResolvedValue({ ok: true });
  calls.cancel.mockResolvedValue({ ok: true });
  calls.open.mockResolvedValue(true);
  calls.copy.mockResolvedValue(true);
});

const press = (container: HTMLElement, label: string) => act(async () => {
  const target = [...container.querySelectorAll<HTMLElement>('[aria-label]')].find((node) => node.getAttribute('aria-label') === label)
    ?? [...container.querySelectorAll<HTMLElement>('button')].find((node) => node.textContent === label);
  expect(target, label).toBeTruthy();
  target!.click();
});
const type = (container: HTMLElement, label: string, value: string) => act(async () => {
  const input = container.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${label}"]`)!;
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
});
const flush = async () => { for (let i = 0; i < 4; i++) await act(async () => Promise.resolve()); };

function mount(props: Partial<Parameters<typeof AntigravityContaLogin>[0]> = {}) {
  const container = document.createElement('div');
  const root = createRoot(container);
  const onComplete = vi.fn();
  const onClose = vi.fn();
  const element = createElement(AntigravityContaLogin, { server, accounts, onComplete, onClose, ...props });
  return { container, root, onComplete, onClose, render: () => act(async () => root.render(element)) };
}

describe('AntigravityContaLogin', () => {
  it('entra na conta da linha: link do Google para abrir e copiar, código colado e conclusão pelo servidor', async () => {
    const view = mount({ accountId: 'trabalho' });
    try {
      await view.render();
      await flush();
      expect(calls.start).toHaveBeenCalledWith(server, 'trabalho');
      expect(calls.read).toHaveBeenCalledWith(server, 'trabalho', 'a1', expect.any(AbortSignal));
      expect(view.container.textContent).toContain(link);
      await press(view.container, m.contas_login_copiar_link());
      expect(calls.copy).toHaveBeenCalledWith(link);
      await press(view.container, m.contas_login_abrir());
      expect(calls.open).toHaveBeenCalledWith(link);
      await type(view.container, m.contas_login_codigo(), ' 4/0AVGzR1A ');
      calls.read.mockResolvedValue({ attempt_id: 'a1', estado: 'concluido', saida: [] });
      await press(view.container, m.mobile_antigravity_send_code());
      await flush();
      expect(calls.submit).toHaveBeenCalledWith(server, 'trabalho', 'a1', '4/0AVGzR1A');
      expect(view.onComplete).toHaveBeenCalledTimes(1);
      expect(view.container.textContent).toContain(m.contas_login_conectada());
    } finally { await act(async () => view.root.unmount()); }
    // Concluída, a tentativa não é cancelada ao fechar.
    expect(calls.cancel).not.toHaveBeenCalled();
  });

  it('não abre nem copia link que não é do login do Google', async () => {
    calls.read.mockResolvedValue({ ...waiting, link: 'https://accounts.google.com.evil.test/o/oauth2/auth' });
    const view = mount({ accountId: 'trabalho' });
    try {
      await view.render();
      await flush();
      await press(view.container, m.contas_login_abrir());
      await press(view.container, m.contas_login_copiar_link());
      expect(calls.open).not.toHaveBeenCalled();
      expect(calls.copy).not.toHaveBeenCalled();
      expect(view.container.textContent).toContain(m.mobile_accounts_claude_invalid_url());
    } finally { await act(async () => view.root.unmount()); }
  });

  it('com a padrão já logada, sugere a adicional e cria a pasta antes do login', async () => {
    calls.create.mockResolvedValue({ id: 'trabalho-x', pasta: '/h/.antigravity-trabalho-x' });
    const view = mount();
    try {
      await view.render();
      expect(calls.start).not.toHaveBeenCalled();
      await type(view.container, m.novacred_nome_conta(), 'Trabalho X');
      await press(view.container, m.contas_entrar());
      await flush();
      expect(calls.create).toHaveBeenCalledWith(server, 'trabalho-x');
      expect(calls.start).toHaveBeenCalledWith(server, 'trabalho-x');
    } finally { await act(async () => view.root.unmount()); }
  });

  it('código recusado fica na tela e a tentativa continua', async () => {
    calls.submit.mockRejectedValue(Object.assign(new Error('código errado'), { status: 422, code: 'erro_codigo_invalido' }));
    const view = mount({ accountId: 'default' });
    try {
      await view.render();
      await flush();
      await type(view.container, m.contas_login_codigo(), 'xyz');
      await press(view.container, m.mobile_antigravity_send_code());
      await flush();
      expect(view.container.textContent).toContain('código errado');
      expect(view.container.textContent).toContain(link);
    } finally { await act(async () => view.root.unmount()); }
    expect(calls.cancel).toHaveBeenCalledWith(server, 'default', 'a1');
  });

  it('Cancelar encerra a tentativa no servidor antes de fechar; falha ao cancelar não fecha', async () => {
    const view = mount({ accountId: 'default' });
    try {
      await view.render();
      await flush();
      calls.cancel.mockRejectedValueOnce(new Error('cancel failed'));
      await press(view.container, m.comum_cancelar());
      await flush();
      expect(view.onClose).not.toHaveBeenCalled();
      expect(view.container.textContent).toContain('cancel failed');
    } finally { await act(async () => view.root.unmount()); }
  });

  it('volta do navegador confere a tentativa na hora', async () => {
    const view = mount({ accountId: 'default' });
    try {
      await view.render();
      await flush();
      const before = calls.read.mock.calls.length;
      await act(async () => calls.foreground?.('active'));
      await flush();
      expect(calls.read.mock.calls.length).toBeGreaterThan(before);
    } finally { await act(async () => view.root.unmount()); }
  });
});
