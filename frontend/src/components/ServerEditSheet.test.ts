// @vitest-environment happy-dom
// Validação do token — mudou do editor inline do ServerManager pra esta folha, junto com o resto da
// edição de servidor (nome + endereço visível + token mascarado).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ServerEditSheet from './ServerEditSheet.svelte';
import * as m from '../paraglide/messages';
import * as auth from '../lib/auth';
import type { Server } from '../lib/auth';

vi.mock('../lib/auth', () => ({
  serverColor: () => '#fff',
  validarPareamento: vi.fn(),
}));
vi.mock('../lib/vaultPush.svelte', () => ({
  vaultPush: { estado: 'idle', detalhe: '', clear: vi.fn() },
}));

const authMock = vi.mocked(auth);
const SRV: Server = { id: 'srv-a', label: 'Casa', baseUrl: 'http://a', token: 'tok-velho' } as Server;

async function montar(props: Record<string, unknown> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ServerEditSheet, {
    target: el,
    props: {
      open: true, server: SRV, onClose: vi.fn(),
      onRename: vi.fn(), onUpdateToken: () => true,
      ...props,
    },
  });
  await tick();   // o $effect que preenche os campos roda depois da montagem
  return { el, comp: comp as never };
}

// A folha vai pro <body> (portal do BottomSheet), então a busca é global.
const campoToken = () =>
  document.querySelector<HTMLInputElement>(
    `input[aria-label="${m.servidor_novo_token_aria({ nome: 'Casa' })}"]`,
  )!;

async function digitarToken(texto: string) {
  const input = campoToken();
  input.value = texto;
  input.dispatchEvent(new Event('input'));
  await tick();
  document.querySelectorAll<HTMLButtonElement>('button').forEach((b) => {
    if (b.textContent?.trim() === m.ctx_salvar()) b.click();
  });
  await tick();
}

beforeEach(() => { document.body.innerHTML = ''; });

describe('ServerEditSheet', () => {
  it('mostra o que está gravado: nome, endereço e token (mascarado)', async () => {
    const t = await montar();
    expect(document.body.textContent).toContain('http://a');
    expect(campoToken().value).toBe('tok-velho');     // trocar SÓ o token: dá pra ver o atual
    expect(campoToken().type).toBe('password');       // ...sem exibir o segredo na tela
    unmount(t.comp);
  });

  it('o olho revela e volta a esconder o token', async () => {
    const t = await montar();
    const olho = document.querySelector<HTMLButtonElement>(
      `button[aria-label="${m.servidor_mostrar_token()}"]`,
    )!;
    olho.click();
    await tick();
    expect(campoToken().type).toBe('text');
    document.querySelector<HTMLButtonElement>(
      `button[aria-label="${m.servidor_ocultar_token()}"]`,
    )!.click();
    await tick();
    expect(campoToken().type).toBe('password');
    unmount(t.comp);
  });

  it('só o nome mudou: renomeia sem tocar no token', async () => {
    const onRename = vi.fn();
    const onUpdateToken = vi.fn(() => true);
    const t = await montar({ onRename, onUpdateToken });
    const nome = document.querySelectorAll<HTMLInputElement>('input')[0];
    nome.value = 'Casa 2';
    nome.dispatchEvent(new Event('input'));
    await tick();
    await digitarToken('tok-velho');
    expect(onRename).toHaveBeenCalledWith('srv-a', 'Casa 2');
    expect(onUpdateToken).not.toHaveBeenCalled();
    expect(authMock.validarPareamento).not.toHaveBeenCalled();  // token igual: nem valida
    unmount(t.comp);
  });

  it('URL inválida recusa sem chamar onUpdateToken e mostra erro ligado ao campo', async () => {
    authMock.validarPareamento.mockReturnValue(null);
    const onUpdateToken = vi.fn(() => true);
    const onClose = vi.fn();
    const t = await montar({ onUpdateToken, onClose });
    await digitarToken('https:// pc.ts.net/?token=abc');
    expect(onUpdateToken).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    const err = document.querySelector<HTMLElement>('#se-err');
    expect(err?.textContent).toContain(m.servidor_url_invalida());
    expect(err?.getAttribute('role')).toBe('alert');
    expect(campoToken().getAttribute('aria-describedby')).toBe('se-err');
    unmount(t.comp);
  });

  it('campo em branco não desautentica o servidor calado', async () => {
    const onUpdateToken = vi.fn(() => true);
    const t = await montar({ onUpdateToken });
    await digitarToken('');
    expect(onUpdateToken).not.toHaveBeenCalled();
    expect(document.querySelector('#se-err')?.textContent).toContain(m.servidor_token_vazio());
    unmount(t.comp);
  });

  it('token cru válido grava e fecha', async () => {
    authMock.validarPareamento.mockReturnValue({ base: '', token: 'tok-novo' });
    const onUpdateToken = vi.fn(() => true);
    const onClose = vi.fn();
    const t = await montar({ onUpdateToken, onClose });
    await digitarToken('tok-novo');
    expect(onUpdateToken).toHaveBeenCalledWith('srv-a', 'tok-novo');
    expect(onClose).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('URL de outro host: grava só o token, avisa e NÃO fecha', async () => {
    authMock.validarPareamento.mockReturnValue({ base: 'https://outra', token: 'tok-outro' });
    const onUpdateToken = vi.fn(() => true);
    const onClose = vi.fn();
    const t = await montar({ onUpdateToken, onClose });
    await digitarToken('https://outra/?token=tok-outro');
    expect(onUpdateToken).toHaveBeenCalledWith('srv-a', 'tok-outro');   // base preservada
    expect(onClose).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain(m.servidor_token_trocado({ url: 'http://a' }));
    unmount(t.comp);
  });

  it('id sumiu entre abrir e salvar: avisa em vez de fechar calado', async () => {
    authMock.validarPareamento.mockReturnValue({ base: '', token: 'tok-novo' });
    const onClose = vi.fn();
    const t = await montar({ onUpdateToken: () => false, onClose });
    await digitarToken('tok-novo');
    expect(onClose).not.toHaveBeenCalled();
    expect(document.querySelector('#se-err')?.textContent).toContain(m.servidor_nao_existe());
    unmount(t.comp);
  });
});
