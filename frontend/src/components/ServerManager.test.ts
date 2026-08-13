// @vitest-environment happy-dom
// Gate do botão Remover (round 1 da 4b): o ramo mobile (sem onSwitchActive) e o ramo com picker só
// mostram × quando sobra mais de 1 servidor OU podeRemoverUltimo=true.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ServerManager from './ServerManager.svelte';
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
const UNICO: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' } as Server;

function montar(props: Partial<Record<string, unknown>> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ServerManager, {
    target: el,
    props: {
      servers: [UNICO],
      onRename: vi.fn(),
      onUpdateToken: () => true,
      onRemove: vi.fn(),
      onAdd: vi.fn(),
      ...props,
    },
  });
  return { el, comp: comp as never };
}

describe('ServerManager — botão Remover', () => {
  it('ramo mobile (sem onSwitchActive) com 1 servidor: × escondido', () => {
    const t = montar();
    expect(t.el.querySelector('.sm-srv-del')).toBeNull();
    unmount(t.comp);
  });

  it('ramo mobile com podeRemoverUltimo: × visível', () => {
    const t = montar({ podeRemoverUltimo: true });
    expect(t.el.querySelector('.sm-srv-del')).not.toBeNull();
    unmount(t.comp);
  });

  it('ramo com picker (onSwitchActive) com 1 servidor: × escondido', () => {
    const t = montar({ onSwitchActive: vi.fn() });
    expect(t.el.querySelector('.sm-srv-pick')).not.toBeNull();
    expect(t.el.querySelector('.sm-srv-del')).toBeNull();
    unmount(t.comp);
  });

  it('ramo com picker + podeRemoverUltimo: × visível', () => {
    const t = montar({ onSwitchActive: vi.fn(), podeRemoverUltimo: true });
    expect(t.el.querySelector('.sm-srv-del')).not.toBeNull();
    unmount(t.comp);
  });
});

// Na tela Servidores das Configurações a linha INTEIRA escolhe onde as configs de servidor serão
// gravadas — e o conceito de "ativo" não aparece. Ele não é escolha do usuário: quem o troca é a
// rota (App.applyRouteServer), a cada sessão aberta, então um botão "ativo" ali era um controle que
// mente (apertar não muda nada visível; o próximo clique numa sessão desfaz).
describe('ServerManager — escolha de alvo das configs', () => {
  const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'y' } as Server;

  it('com onPickTarget: clicar na linha escolhe o alvo', () => {
    const onPickTarget = vi.fn();
    const t = montar({ servers: [UNICO, B], onPickTarget, targetId: 'srv-a' });
    const linhas = t.el.querySelectorAll<HTMLButtonElement>('.sm-srv-pick');
    expect(linhas.length).toBe(2);
    linhas[1].click();
    expect(onPickTarget).toHaveBeenCalledWith('srv-b');
    unmount(t.comp);
  });

  it('com onPickTarget: o marcado é o ALVO, e nenhuma linha diz "ativo"', () => {
    // activeId aponta pro OUTRO servidor de propósito: se o destaque seguisse o ativo, a tela
    // marcaria a máquina errada como destino das configs.
    const t = montar({ servers: [UNICO, B], onPickTarget: vi.fn(), targetId: 'srv-b', activeId: 'srv-a' });
    const tags = [...t.el.querySelectorAll('.sm-tag')].map((n) => n.textContent);
    expect(tags).toEqual(['escolhido']);
    expect(t.el.textContent).not.toContain('ativo');
    const marcadas = [...t.el.querySelectorAll('.sm-srv.on .sm-srv-label')].map((n) => n.textContent);
    expect(marcadas).toEqual(['B']);
    unmount(t.comp);
  });

  it('sem onPickTarget (menu de conta): segue trocando o ativo', () => {
    const onSwitchActive = vi.fn();
    const t = montar({ servers: [UNICO, B], onSwitchActive, activeId: 'srv-a' });
    expect([...t.el.querySelectorAll('.sm-tag')].map((n) => n.textContent)).toEqual(['ativo']);
    t.el.querySelectorAll<HTMLButtonElement>('.sm-srv-pick')[1].click();
    expect(onSwitchActive).toHaveBeenCalledWith('srv-b');
    unmount(t.comp);
  });
});

describe('ServerManager — semântica ARIA do botão Adicionar (round 7)', () => {
  it('default (Settings): botão COMUM, sem role=menuitem (não há ancestral role=menu)', () => {
    const t = montar();
    const add = t.el.querySelector<HTMLButtonElement>('.sm-item')!;
    expect(add).not.toBeNull();
    expect(add.getAttribute('role')).toBeNull();
    expect(add.getAttribute('type')).toBe('button');
    unmount(t.comp);
  });

  it('menuitem=true (AccountMenu popover, dentro de role=menu): papel preservado', () => {
    const t = montar({ menuitem: true });
    const add = t.el.querySelector<HTMLButtonElement>('.sm-item')!;
    expect(add.getAttribute('role')).toBe('menuitem');
    unmount(t.comp);
  });
});

describe('ServerManager — saveToken validado (round 4)', () => {
  async function editarToken(t: { el: HTMLElement }, texto: string) {
    t.el.querySelector<HTMLButtonElement>('.sm-srv-rename[aria-label="Trocar token de A"]')!.click();
    await tick();   // editor inline só monta depois do re-render
    const input = t.el.querySelector<HTMLInputElement>('.sm-srv-edit')!;
    input.value = texto;
    input.dispatchEvent(new Event('input'));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
  }

  it('URL inválida recusa sem chamar onUpdateToken e mostra erro role=alert ligado ao campo', async () => {
    authMock.validarPareamento.mockReturnValue(null);
    const onUpdateToken = vi.fn(() => true);
    const t = montar({ onUpdateToken });
    await editarToken(t, 'https:// pc.ts.net/?token=abc');
    expect(authMock.validarPareamento).toHaveBeenCalledWith('https:// pc.ts.net/?token=abc', { aceitarTokenCru: true });
    expect(onUpdateToken).not.toHaveBeenCalled();
    const err = t.el.querySelector<HTMLElement>('#sm-token-err');
    expect(err?.innerText).toContain('URL de pareamento inválida');
    expect(err?.getAttribute('role')).toBe('alert');
    const input = t.el.querySelector<HTMLInputElement>('.sm-srv-edit')!;
    expect(input.getAttribute('aria-invalid')).toBe('true');
    expect(input.getAttribute('aria-describedby')).toBe('sm-token-err');
    expect(document.activeElement).toBe(input);   // erro associado ao campo: foco onde corrigir
    unmount(t.comp);
  });

  it('token cru válido (aceitarTokenCru) chama onUpdateToken com o token', async () => {
    authMock.validarPareamento.mockReturnValue({ base: '', token: 'tok-novo' });
    const onUpdateToken = vi.fn(() => true);
    const t = montar({ onUpdateToken });
    await editarToken(t, 'tok-novo');
    expect(onUpdateToken).toHaveBeenCalledTimes(1);
    expect(onUpdateToken).toHaveBeenCalledWith(UNICO.id, 'tok-novo');
    unmount(t.comp);
  });

  it('URL de outro host não reaponta o servidor: onUpdateToken recebe só o token', async () => {
    authMock.validarPareamento.mockReturnValue({ base: 'https://outra', token: 'tok-outro' });
    const onUpdateToken = vi.fn(() => true);
    const t = montar({ onUpdateToken });
    await editarToken(t, 'https://outra/?token=tok-outro');
    expect(onUpdateToken).toHaveBeenCalledWith(UNICO.id, 'tok-outro');   // token só, base preservada
    unmount(t.comp);
  });
});
