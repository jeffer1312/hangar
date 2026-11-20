// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CliproxyDeteccao, CliproxyInstalacao, Server } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { overwriteGetLocale } from '../../paraglide/runtime';
import { CliproxyWizard } from './CliproxyWizard';

const calls = vi.hoisted(() => ({
  detect: vi.fn(), install: vi.fn(), login: vi.fn(), status: vi.fn(), callback: vi.fn(), key: vi.fn(), reset: vi.fn(), put: vi.fn(), open: vi.fn(),
  push: vi.fn(),
}));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: calls.push }) }));
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  engineCliproxyForServer: calls.detect, installCliproxyForServer: calls.install, startCliproxyLoginForServer: calls.login,
  getCliproxyLoginForServer: calls.status, sendCliproxyLoginCallbackForServer: calls.callback,
  putCliproxyManagementKeyForServer: calls.key, resetCliproxyManagementKeyForServer: calls.reset, putEngineForServer: calls.put,
}));
vi.mock('expo-linking', () => ({ openURL: calls.open }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: ReactNode }) => createElement('div', { role: 'dialog' }, children) }));
vi.mock('react-native', async original => ({
  ...await original<typeof import('react-native')>(),
  TextInput: (props: { accessibilityLabel: string; value: string; onChangeText: (value: string) => void }) => createElement('textarea', {
    'aria-label': props.accessibilityLabel, value: props.value,
    onInput: (event: { currentTarget: { value: string } }) => props.onChangeText(event.currentTarget.value),
  }),
  Switch: (props: { accessibilityLabel: string; value: boolean; onValueChange: (v: boolean) => void }) => createElement('input', {
    type: 'checkbox', 'aria-label': props.accessibilityLabel, checked: props.value,
    onChange: (event: { currentTarget: { checked: boolean } }) => props.onValueChange(event.currentTarget.checked),
  }),
}));
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
overwriteGetLocale(() => 'pt');

const server: Server = { id: 'b', label: 'B', baseUrl: 'https://b.local', token: 'token-b' };
const install = (patch: Partial<CliproxyInstalacao> = {}): CliproxyInstalacao =>
  ({ state: 'managed', version: 'v1', latest: 'v1', update_available: false, running: true, job: null, ...patch });
const proxy = (patch: Partial<CliproxyDeteccao> = {}): CliproxyDeteccao => ({
  found: true, base_url: 'http://127.0.0.1:8317/v1', models: [{ id: 'claude-sonnet-x', context_length: null, vision: null }, { id: 'gpt-6.1-sol', context_length: 872000, vision: null }], error: null,
  connected: [{ type: 'codex', email: 'velho@x.com' }], install: install(), ...patch,
});
const roots: Array<ReturnType<typeof createRoot>> = [];
beforeEach(() => {
  for (const fn of Object.values(calls)) fn.mockReset();
  calls.detect.mockResolvedValue(proxy());
  calls.login.mockResolvedValue({ url: 'https://auth.openai.com/authorize?state=st1', state: 'st1' });
  calls.status.mockResolvedValue({ status: 'wait' });
  calls.callback.mockResolvedValue({ status: 'sent' });
  calls.put.mockResolvedValue({ motores: {} });
  calls.open.mockResolvedValue(true);
});
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  vi.useRealTimers();
});

// O login encadeia vários pedidos já resolvidos; com timers falsos não dá para esperar um setTimeout.
async function settle() { for (let i = 0; i < 20; i++) await Promise.resolve(); }
async function show(names: string[] = []) {
  const container = document.createElement('div'); const root = createRoot(container); roots.push(root);
  const onSaved = vi.fn(); const onClose = vi.fn();
  await act(async () => { root.render(createElement(CliproxyWizard, { server, names, onSaved, onClose })); await settle(); });
  return { container, onSaved, onClose };
}
function button(container: HTMLElement, label: string) {
  return [...container.querySelectorAll('button')].find(b => b.getAttribute('aria-label') === label);
}
async function click(container: HTMLElement, label: string) {
  const target = button(container, label); expect(target, label).toBeTruthy();
  await act(async () => { target!.click(); await settle(); });
}
async function fill(container: HTMLElement, label: string, value: string) {
  const target = container.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${label}"]`); expect(target, label).toBeTruthy();
  await act(async () => { target!.value = value; target!.dispatchEvent(new Event('input', { bubbles: true })); });
}

describe('assistente do CLIProxyAPI no celular', () => {
  it('proxy já instalado: entra na conta colando o endereço e cria o motor com Fast', async () => {
    const view = await show(['outro']);
    expect(view.container.textContent).toContain('Qual conta você quer usar?');
    expect(calls.install).not.toHaveBeenCalled();

    await click(view.container, 'ChatGPT, Recomendado');
    expect(calls.login).toHaveBeenCalledExactlyOnceWith(server, 'codex');
    expect(calls.open).toHaveBeenCalledWith('https://auth.openai.com/authorize?state=st1');
    expect(view.container.textContent).toContain('não foi possível conectar');

    await fill(view.container, 'Endereço da barra do navegador', 'https://chatgpt.com/');
    await click(view.container, 'Concluir login');
    expect(calls.callback).not.toHaveBeenCalled();
    expect(view.container.textContent).toContain('não parece o endereço de volta');

    calls.status.mockResolvedValue({ status: 'ok' });
    calls.detect.mockResolvedValue(proxy({ connected: [{ type: 'codex', email: 'velho@x.com' }, { type: 'codex', email: 'novo@x.com' }] }));
    const callback = 'http://localhost:1455/auth/callback?code=abc&state=st1';
    await fill(view.container, 'Endereço da barra do navegador', callback);
    await click(view.container, 'Concluir login');
    expect(calls.callback).toHaveBeenCalledExactlyOnceWith(server, 'st1', callback);
    expect(view.container.textContent).toContain('Conta conectada.');
    expect(view.container.textContent).toContain('novo@x.com');

    await click(view.container, 'Continuar');
    expect(view.container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Nome"]')!.value).toBe('GPT novo');
    expect(button(view.container, 'claude-sonnet-x')).toBeUndefined();
    expect(view.container.querySelector<HTMLInputElement>(`input[aria-label="${m.mobile_cliproxy_wizard_fast()}"]`)!.checked).toBe(true);

    await click(view.container, 'Concluir');
    expect(calls.put).toHaveBeenCalledExactlyOnceWith(server, 'gpt-novo', {
      label: 'GPT novo', base_url: 'http://127.0.0.1:8317/v1', model: 'gpt-6.1-sol', subagent_model: 'gpt-6.1-sol',
      use_cliproxy_key: true, experimental_betas: true, fast: true, context_window: 828400,
    });
    expect(view.onSaved).toHaveBeenCalledTimes(1);
    expect(view.container.textContent).toContain('Tudo pronto');

    await click(view.container, 'Abrir sessão com GPT novo');
    expect(view.onClose).toHaveBeenCalledTimes(1);
    expect(calls.push).toHaveBeenCalledExactlyOnceWith({ pathname: '/create', params: { server: 'b', engine: 'gpt-novo' } });
  });

  it('login com erro para de consultar; colar o endereço volta a consultar e conecta', async () => {
    vi.useFakeTimers();
    calls.status.mockResolvedValue({ status: 'error', error: 'negado' });
    const view = await show();
    await click(view.container, 'ChatGPT, Recomendado');
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); await settle(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(6000); await settle(); });
    expect(calls.status).toHaveBeenCalledTimes(1);

    calls.status.mockResolvedValue({ status: 'ok' });
    calls.detect.mockResolvedValue(proxy({ connected: [{ type: 'codex', email: 'velho@x.com' }, { type: 'codex', email: 'novo@x.com' }] }));
    await fill(view.container, 'Endereço da barra do navegador', 'http://localhost:1455/auth/callback?code=abc&state=st1');
    await click(view.container, 'Concluir login');
    expect(view.container.textContent).toContain('Conta conectada.');
  });

  it('conta que já está no proxy segue para o nome sem abrir login', async () => {
    const view = await show();
    expect(view.container.textContent).toContain('velho@x.com');
    await click(view.container, 'Usar esta conta');
    expect(calls.login).not.toHaveBeenCalled();
    expect(view.container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Nome"]')!.value).toBe('GPT velho');
  });

  it.each([
    ['proxy rodando sem instalação conhecida vai direto à conta', install({ state: 'absent', version: null }), true],
    ['proxy do Hangar parado pede a instalação', install({ running: false }), false],
  ])('%s', async (_, setup, direct) => {
    calls.detect.mockResolvedValue(proxy({ install: setup }));
    const view = await show();
    expect(view.container.textContent?.includes('Qual conta você quer usar?')).toBe(direct);
  });

  it('recusa nome vazio e nome de motor que já existe sem gravar', async () => {
    calls.status.mockResolvedValue({ status: 'ok' });
    const view = await show(['gpt-velho']);
    await click(view.container, 'ChatGPT, Recomendado');
    await fill(view.container, 'Endereço da barra do navegador', 'http://127.0.0.1:1455/auth/callback?state=st1');
    await click(view.container, 'Concluir login');
    await click(view.container, 'Continuar');
    await fill(view.container, 'Nome', '  ');
    await click(view.container, 'Concluir');
    expect(view.container.textContent).toContain('Dê um nome para achar o motor depois.');
    await fill(view.container, 'Nome', 'GPT velho');
    await click(view.container, 'Concluir');
    expect(view.container.textContent).toContain('Já existe um motor com este nome.');
    expect(calls.put).not.toHaveBeenCalled();
  });

  it('pede a senha de gerenciamento quando falta e abre o login depois de salvar', async () => {
    calls.login.mockRejectedValueOnce(Object.assign(new Error('409: '), { status: 409, code: 'erro_cliproxy_sem_senha' }));
    calls.key.mockResolvedValue({ unnamed_accounts: 0, management_key_set: true, naming_error: null });
    const view = await show();
    await click(view.container, 'ChatGPT, Recomendado');
    expect(view.container.textContent).toContain('senha de gerenciamento');
    expect(calls.open).not.toHaveBeenCalled();
    await fill(view.container, 'Senha de gerenciamento do CLIProxyAPI', 'segredo');
    await click(view.container, 'Salvar e entrar');
    expect(calls.key).toHaveBeenCalledExactlyOnceWith(server, 'segredo');
    expect(calls.login).toHaveBeenCalledTimes(2);
    expect(calls.open).toHaveBeenCalledTimes(1);
  });

  it('sem saber a senha, troca por uma nova e abre o login', async () => {
    calls.login.mockRejectedValueOnce(Object.assign(new Error('409: '), { status: 409, code: 'erro_cliproxy_sem_senha' }));
    calls.reset.mockResolvedValue({ status: 'reset' });
    const view = await show();
    await click(view.container, 'ChatGPT, Recomendado');
    await click(view.container, 'Usar uma senha nova');
    expect(calls.reset).toHaveBeenCalledExactlyOnceWith(server);
    expect(calls.key).not.toHaveBeenCalled();
    expect(calls.login).toHaveBeenCalledTimes(2);
    expect(calls.open).toHaveBeenCalledTimes(1);
  });

  it('instala do zero acompanhando a tarefa e mostra a falha do download com Tentar de novo', async () => {
    vi.useFakeTimers();
    calls.detect.mockResolvedValueOnce(proxy({ found: false, base_url: null, models: [], connected: [], install: install({ state: 'absent', version: null }) }));
    calls.install.mockResolvedValue({ job: { kind: 'install', step: 'download', done: false, error: null, outcome: null } });
    const view = await show();
    expect(view.container.textContent).toContain('Baixar o CLIProxyAPI');
    await click(view.container, 'Instalar');
    expect(calls.install).toHaveBeenCalledExactlyOnceWith(server);
    calls.detect.mockResolvedValueOnce(proxy({ found: false, install: install({ state: 'absent',
      job: { kind: 'install', step: 'download', done: true, error: { code: 'cliproxy_download_failed', detail: 'timeout' }, outcome: null } }) }));
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); await settle(); });
    expect(view.container.textContent).toContain('Não consegui baixar o CLIProxyAPI do GitHub');
    expect(button(view.container, 'Tentar de novo')).toBeTruthy();
  });

  it('proxy de outra origem: Continuar só completa o config e segue para a conta', async () => {
    vi.useFakeTimers();
    calls.detect.mockResolvedValueOnce(proxy({ install: install({ state: 'external', version: null }) }));
    calls.install.mockResolvedValue({ job: { kind: 'install', step: 'config', done: false, error: null, outcome: null } });
    const view = await show();
    expect(view.container.textContent).toContain('Nada foi reinstalado');
    await click(view.container, 'Continuar');
    calls.detect.mockResolvedValueOnce(proxy({ install: install({ state: 'external',
      job: { kind: 'install', step: 'start', done: true, error: null, outcome: 'configured' } }) }));
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); await settle(); });
    expect(view.container.textContent).toContain('Qual conta você quer usar?');
  });
});
