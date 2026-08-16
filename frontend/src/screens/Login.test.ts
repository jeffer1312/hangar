// @vitest-environment happy-dom
// Round 4 da 4b: validação ESTRITA ANTES de addServer — URL/token inválidos não mutam storage nem
// navegam (erro visível com retry); válidos chegam parseados ao addServer.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import Login from './Login.svelte';
import * as auth from '../lib/auth';
import * as api from '../lib/api';
import * as sync from '../lib/sync';
import { overwriteGetLocale } from '../paraglide/runtime';

beforeEach(() => overwriteGetLocale(() => 'pt'));


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

beforeEach(() => { vi.clearAllMocks(); });   // contagens de chamada não vazam entre testes

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

// Round 5: o deep-link é validado pela URL COMPLETA antes de extrair qualquer coisa — URLSearchParams
// descartava duplicatas e api vazia, e o validator via uma URL reconstruída (token/api duplicados
// passavam). Agora o validator recebe a href inteira; inválido = sem preencher campos, sem limpar a
// URL, sem conectar; válido = limpa o histórico DEPOIS da validação.
describe('Login — deep-link validado pela URL completa (round 5)', () => {
  async function montarComUrl(url: string) {
    // happy-dom expõe a troca de URL em window.happyDOM (fora dos tipos TS do navegador)
    (window as unknown as { happyDOM: { setURL: (u: string) => void } }).happyDOM.setURL(url);
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(Login, { target: el, props: { onLogin: vi.fn() } }) as never;
    await tick(); await tick();   // onMount async (syncStatus) + handler do deep-link
    return { el, comp };
  }

  it('token duplicado: validator recebe a URL COMPLETA, nada conecta, URL não é limpa, erro associado aos campos', async () => {
    const replaceState = vi.spyOn(window.history, 'replaceState');
    authMock.validarPareamento.mockReturnValue(null);
    const t = await montarComUrl('https://casa.ts.net/?token=abc&token=def');
    expect(authMock.validarPareamento).toHaveBeenCalledWith('https://casa.ts.net/?token=abc&token=def');
    expect(authMock.addServerWithRollback).not.toHaveBeenCalled();
    expect(replaceState).not.toHaveBeenCalled();
    const err = t.el.querySelector<HTMLElement>('.error-msg');
    expect(err?.innerText).toContain('Deep-link');
    expect(err?.getAttribute('role')).toBe('alert');
    // round 6: erro associado aos campos (aria-invalid/describedby) + foco no primeiro inválido
    const url = t.el.querySelector<HTMLInputElement>('#base-url')!;
    const tok = t.el.querySelector<HTMLInputElement>('#token')!;
    expect(url.getAttribute('aria-invalid')).toBe('true');
    expect(tok.getAttribute('aria-invalid')).toBe('true');
    expect(url.getAttribute('aria-describedby')).toBe('login-err');
    expect(tok.getAttribute('aria-describedby')).toBe('login-err');
    expect(document.activeElement).toBe(url);
    unmount(t.comp);
  });

  it('api duplicada: rejeita sem conectar nem limpar, erro associado aos campos', async () => {
    const replaceState = vi.spyOn(window.history, 'replaceState');
    authMock.validarPareamento.mockReturnValue(null);
    const t = await montarComUrl('https://casa.ts.net/?token=abc&api=https://a.example&api=https://b.example');
    expect(authMock.validarPareamento).toHaveBeenCalledWith(
      'https://casa.ts.net/?token=abc&api=https://a.example&api=https://b.example',
    );
    expect(authMock.addServerWithRollback).not.toHaveBeenCalled();
    expect(replaceState).not.toHaveBeenCalled();
    expect(t.el.querySelector<HTMLInputElement>('#base-url')!.getAttribute('aria-invalid')).toBe('true');
    expect(t.el.querySelector<HTMLInputElement>('#token')!.getAttribute('aria-describedby')).toBe('login-err');
    unmount(t.comp);
  });

  it('api vazia: rejeita sem conectar nem limpar, erro associado aos campos', async () => {
    const replaceState = vi.spyOn(window.history, 'replaceState');
    authMock.validarPareamento.mockReturnValue(null);
    const t = await montarComUrl('https://casa.ts.net/?token=abc&api=');
    expect(authMock.validarPareamento).toHaveBeenCalledWith('https://casa.ts.net/?token=abc&api=');
    expect(authMock.addServerWithRollback).not.toHaveBeenCalled();
    expect(replaceState).not.toHaveBeenCalled();
    expect(t.el.querySelector<HTMLInputElement>('#base-url')!.getAttribute('aria-invalid')).toBe('true');
    expect(t.el.querySelector<HTMLInputElement>('#token')!.getAttribute('aria-describedby')).toBe('login-err');
    expect(document.activeElement).toBe(t.el.querySelector<HTMLInputElement>('#base-url'));
    unmount(t.comp);
  });

  it('deep-link válido: valida a URL completa, conecta UMA vez e limpa o histórico depois da validação', async () => {
    const replaceState = vi.spyOn(window.history, 'replaceState');
    authMock.validarPareamento.mockReturnValue({ base: 'https://casa.ts.net', token: 'abc' });
    authMock.addServerWithRollback.mockResolvedValue({ id: 'srv-x', succeeded: true });
    const t = await montarComUrl('https://casa.ts.net/?token=abc');
    expect(authMock.validarPareamento).toHaveBeenCalledWith('https://casa.ts.net/?token=abc');
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(1);
    expect(replaceState).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('visita normal (sem token na URL) não é deep-link: nada de erro nem de conexão', async () => {
    const replaceState = vi.spyOn(window.history, 'replaceState');
    const t = await montarComUrl('https://casa.ts.net/');
    expect(authMock.validarPareamento).not.toHaveBeenCalled();
    expect(authMock.addServerWithRollback).not.toHaveBeenCalled();
    expect(replaceState).not.toHaveBeenCalled();
    expect(t.el.querySelector('.error-msg')).toBeNull();
    unmount(t.comp);
  });
});

// Task 16: página servida pelo PRÓPRIO backend (Electron, navegador local) nasce com o campo de
// URL preenchido com a origem — o usuário só digita o token. Sem servidor salvo, a origem vence;
// com servidor salvo, o salvo vence; origem do Vite (5173) NÃO preenche (o preview proxya /api,
// mas a origem dele não é o backend — preencher daria erro de conexão com cara de bug).
describe('Login — origem do próprio servidor preenche o campo (task 16)', () => {
  async function montarEm(url: string) {
    (window as unknown as { happyDOM: { setURL: (u: string) => void } }).happyDOM.setURL(url);
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(Login, { target: el, props: { onLogin: vi.fn() } }) as never;
    await tick();
    return { el, comp };
  }

  it('sem servidor salvo e origem do backend: campo nasce preenchido com a origem', async () => {
    authMock.getBaseUrl.mockReturnValue('');
    const t = await montarEm('http://127.0.0.1:8765/');
    expect(t.el.querySelector<HTMLInputElement>('#base-url')!.value).toBe('http://127.0.0.1:8765');
    unmount(t.comp);
  });

  it('com servidor salvo: o salvo vence, mesmo com a origem sendo o backend', async () => {
    authMock.getBaseUrl.mockReturnValue('http://casa:8765');
    const t = await montarEm('http://127.0.0.1:8765/');
    expect(t.el.querySelector<HTMLInputElement>('#base-url')!.value).toBe('http://casa:8765');
    unmount(t.comp);
  });

  it('origem do Vite (5173): campo fica vazio', async () => {
    authMock.getBaseUrl.mockReturnValue('');
    const t = await montarEm('http://127.0.0.1:5173/');
    expect(t.el.querySelector<HTMLInputElement>('#base-url')!.value).toBe('');
    unmount(t.comp);
  });
});
