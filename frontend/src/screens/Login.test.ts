// @vitest-environment happy-dom
// Round 4 da 4b: validação ESTRITA ANTES de addServer — URL/token inválidos não mutam storage nem
// navegam (erro visível com retry); válidos chegam parseados ao addServer.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import Login from './Login.svelte';
import * as auth from '../lib/auth';
import * as api from '../lib/api';
import * as sync from '../lib/sync';

vi.mock('../lib/auth', () => ({
  addServerWithRollback: vi.fn(async () => ({ id: 'srv-x', succeeded: true })),
  getBaseUrl: vi.fn(() => ''),
  validarPareamento: vi.fn(),
}));
vi.mock('../lib/api', () => ({ getSessions: vi.fn(async () => []) }));
vi.mock('../lib/sync', () => ({
  syncStatus: vi.fn(async () => ({ enabled: false })),
  register: vi.fn(),
  login: vi.fn(),
}));

const authMock = vi.mocked(auth);
const apiMock = vi.mocked(api);

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(Login, { target: el, props: { onLogin: vi.fn() } });
  return { el, comp: comp as never };
}

async function preencher(t: { el: HTMLElement }, base: string, token: string) {
  const set = (sel: string, v: string) => {
    const input = t.el.querySelector<HTMLInputElement>(sel)!;
    input.value = v;
    input.dispatchEvent(new Event('input'));
  };
  set('#base-url', base);
  set('#token', token);
  await tick();
}

describe('Login — validação estrita antes do add transacional (round 4)', () => {
  it('URL/token inválidos: addServerWithRollback não é chamado, erro role=alert ligado aos campos', async () => {
    authMock.validarPareamento.mockReturnValue(null);
    const t = montar();
    await preencher(t, 'ftp://host', 'ab cd');
    t.el.querySelector<HTMLButtonElement>('.connect-btn')!.click();
    await tick();
    expect(authMock.addServerWithRollback).not.toHaveBeenCalled();
    const err = t.el.querySelector<HTMLElement>('#login-err');
    expect(err?.innerText).toContain('inválidos');
    expect(err?.getAttribute('role')).toBe('alert');
    // campos ligados ao erro (aria-describedby resolve pro ID) + foco no primeiro inválido
    const url = t.el.querySelector<HTMLInputElement>('#base-url')!;
    const tok = t.el.querySelector<HTMLInputElement>('#token')!;
    expect(url.getAttribute('aria-invalid')).toBe('true');
    expect(tok.getAttribute('aria-invalid')).toBe('true');
    expect(url.getAttribute('aria-describedby')).toBe('login-err');
    expect(tok.getAttribute('aria-describedby')).toBe('login-err');
    expect(document.activeElement).toBe(url);
    unmount(t.comp);
  });

  it('erro de REDE é visível mas NÃO marca campo indevidamente', async () => {
    authMock.validarPareamento.mockReturnValue({ base: 'http://host:8765', token: 'abc' });
    authMock.addServerWithRollback.mockRejectedValue(new Error('servidor fora do ar'));
    const t = montar();
    await preencher(t, 'http://host:8765', 'abc');
    t.el.querySelector<HTMLButtonElement>('.connect-btn')!.click();
    await tick(); await tick();
    const err = t.el.querySelector<HTMLElement>('.error-msg');
    expect(err?.innerText).toContain('servidor fora do ar');
    expect(t.el.querySelector<HTMLInputElement>('#base-url')!.getAttribute('aria-invalid')).toBeNull();
    expect(t.el.querySelector<HTMLInputElement>('#token')!.getAttribute('aria-invalid')).toBeNull();
    unmount(t.comp);
  });

  it('URL/token válidos: addServerWithRollback recebe base+token parseados e conecta', async () => {
    authMock.validarPareamento.mockReturnValue({ base: 'http://host:8765', token: 'abc' });
    authMock.addServerWithRollback.mockResolvedValue({ id: 'srv-x', succeeded: true });   // reset do teste anterior
    apiMock.getSessions.mockResolvedValue([]);
    const onLogin = vi.fn();
    const t = montar();
    // mock de sync já no escopo; a montagem acima usou o onLogin padrão — remonta com o spy
    unmount(t.comp);
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(Login, { target: el, props: { onLogin } }) as never;
    await preencher({ el } as never, 'http://host:8765', 'abc');
    el.querySelector<HTMLButtonElement>('.connect-btn')!.click();
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledWith(
      'http://host:8765', 'abc', expect.any(Function),
    );
    expect(onLogin).toHaveBeenCalled();
    unmount(comp);
  });
});
