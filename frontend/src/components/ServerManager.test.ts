// @vitest-environment happy-dom
// Gate do botão Remover (round 1 da 4b): o ramo mobile (sem onSwitchActive) e o ramo com picker só
// mostram × quando sobra mais de 1 servidor OU podeRemoverUltimo=true.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import ServerManager from './ServerManager.svelte';
import * as m from '../paraglide/messages';
import type { Server } from '../lib/auth';

vi.mock('../lib/auth', () => ({
  serverColor: () => '#fff',
  validarPareamento: vi.fn(),
}));
vi.mock('../lib/vaultPush.svelte', () => ({
  vaultPush: { estado: 'idle', detalhe: '', clear: vi.fn() },
}));

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
    expect(tags).toEqual([m.servidor_escolhido()]);
    expect(t.el.textContent).not.toContain(m.servidor_ativo());
    const marcadas = [...t.el.querySelectorAll('.sm-srv.on .sm-srv-label')].map((n) => n.textContent);
    expect(marcadas).toEqual(['B']);
    unmount(t.comp);
  });

  it('sem onPickTarget (menu de conta): segue trocando o ativo', () => {
    const onSwitchActive = vi.fn();
    const t = montar({ servers: [UNICO, B], onSwitchActive, activeId: 'srv-a' });
    expect([...t.el.querySelectorAll('.sm-tag')].map((n) => n.textContent)).toEqual([m.servidor_ativo()]);
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
