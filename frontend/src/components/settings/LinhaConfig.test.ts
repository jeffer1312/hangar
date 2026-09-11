// @vitest-environment happy-dom
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import LinhaConfig from './LinhaConfig.svelte';
import * as m from '../../paraglide/messages';
import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';

function store(campos: Record<string, unknown>, valor: unknown = '') {
  return {
    get campos() { return campos; },
    valorAtual: () => valor,
    rascunhoDe: () => '',
    setRascunho: vi.fn(),
  } as unknown as ConfigServidorStore;
}

function montar(props: Record<string, unknown>) {
  const alvo = document.createElement('div');
  document.body.appendChild(alvo);
  const app = mount(LinhaConfig, { target: alvo, props: props as never });
  return { alvo, app };
}

describe('LinhaConfig', () => {
  it('segredo já definido mostra a máscara e o campo vazio', () => {
    const { alvo, app } = montar({
      campo: { chave: 'k', rotulo: 'Chave', ajuda: 'ajuda', tipo: 'segredo' },
      store: store({ k: { definido: true, valor: 'sk-•••1234' } }),
    });
    expect(alvo.textContent).toContain('sk-•••1234');
    expect(alvo.querySelector<HTMLInputElement>('input[type="text"]')!.value).toBe('');
    unmount(app);
  });

  it('interruptor reflete o valor atual', () => {
    const { alvo, app } = montar({
      campo: { chave: 'a', rotulo: 'Automações', ajuda: 'ajuda', tipo: 'liga' },
      store: store({ a: {} }, true),
    });
    expect(alvo.querySelector<HTMLInputElement>('input[type="checkbox"]')!.checked).toBe(true);
    unmount(app);
  });

  it('sem dizer o escopo, a linha diz que grava no servidor — é quem usa esta linha', () => {
    const { alvo, app } = montar({
      campo: { chave: 'a', rotulo: 'Automações', ajuda: 'ajuda', tipo: 'liga' },
      store: store({ a: {} }, true),
    });
    expect(alvo.querySelector('label.rot')!.textContent).toContain(m.config_escopo_servidor());
    unmount(app);
  });

  it('escopo do .env troca o texto da etiqueta', () => {
    const { alvo, app } = montar({
      campo: { chave: 'a', rotulo: 'Automações', ajuda: 'ajuda', tipo: 'liga' },
      store: store({ a: {} }, true), escopo: 'env',
    });
    expect(alvo.textContent).toContain(m.config_escopo_env());
    expect(alvo.textContent).not.toContain(m.config_escopo_servidor());
    unmount(app);
  });

  it('escopo nulo não mostra etiqueta nenhuma: ausência é a informação', () => {
    const { alvo, app } = montar({
      campo: { chave: 'a', rotulo: 'Automações', ajuda: 'ajuda', tipo: 'liga' },
      store: store({ a: {} }, true), escopo: null,
    });
    expect(alvo.querySelector('.escopo')).toBeNull();
    expect(alvo.textContent).not.toContain(m.config_escopo_servidor());
    unmount(app);
  });

  it('veredito e motivo viram o "por quê?" que expande; sem os dois, nada aparece', () => {
    const com = montar({
      campo: { chave: 'a', rotulo: 'Automações', ajuda: 'ajuda', tipo: 'liga' },
      store: store({ a: {} }, true), veredito: 'Recomendado: ligado', motivo: 'Encadeia a próxima tarefa.',
    });
    const det = com.alvo.querySelector<HTMLDetailsElement>('details')!;
    expect(det.textContent).toContain('Recomendado: ligado');
    expect(det.textContent).toContain(m.config_motores_por_que());
    expect(det.open).toBe(false);
    expect(det.textContent).toContain('Encadeia a próxima tarefa.');
    unmount(com.app);

    const sem = montar({
      campo: { chave: 'a', rotulo: 'Automações', ajuda: 'ajuda', tipo: 'liga' },
      store: store({ a: {} }, true), veredito: 'Recomendado: ligado',
    });
    expect(sem.alvo.querySelector('details')).toBeNull();
    unmount(sem.app);
  });
});
