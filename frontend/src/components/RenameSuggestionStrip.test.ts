// @vitest-environment happy-dom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import type { Server } from '@hangar/core';
import * as core from '@hangar/core';
import RenameSuggestionStrip from './RenameSuggestionStrip.svelte';

vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  renameSession: vi.fn(async (_old: string, nv: string) => ({ ok: true, name: nv })),
  dismissRenameSuggestion: vi.fn(async () => {}),
  requestRenameSuggestion: vi.fn(async () => ({ suggestion: 'etiquetas-por-maquina', weak: false })),
}));

const server = { id: 's1', label: 's1', baseUrl: 'http://s1', token: 't' } as unknown as Server;
let app: ReturnType<typeof mount> | null = null;

async function flush() {
  for (let i = 0; i < 5; i++) { await Promise.resolve(); await tick(); }
}

function render(weak = false) {
  const calls = { renamed: [] as string[], dismissed: 0, suggested: [] as core.RenameSuggestionResult[] };
  const target = document.createElement('div');
  document.body.appendChild(target);
  app = mount(RenameSuggestionStrip, { target, props: {
    server, sessionName: 'hangar-2', suggestion: 'etiquetas-de-conta', weak,
    onRenamed: (n: string) => calls.renamed.push(n), onDismissed: () => { calls.dismissed++; },
    onSuggested: (s: core.RenameSuggestionResult) => calls.suggested.push(s),
  } });
  const [again, dismiss, apply] = target.querySelectorAll('button');
  return { target, calls, again, dismiss, apply };
}

afterEach(() => { if (app) unmount(app); app = null; document.body.innerHTML = ''; vi.clearAllMocks(); });

describe('RenameSuggestionStrip', () => {
  it('Renomear usa a rota de renomear no servidor da sessão e devolve o nome final', async () => {
    const { calls, apply, target } = render();
    expect(target.textContent).toContain('etiquetas-de-conta');
    apply.click();
    await flush();
    expect(core.renameSession).toHaveBeenCalledWith('hangar-2', 'etiquetas-de-conta', server);
    expect(calls.renamed).toEqual(['etiquetas-de-conta']);
  });

  it('Agora não guarda a recusa e esconde', async () => {
    const { calls, dismiss } = render();
    dismiss.click();
    await flush();
    expect(core.dismissRenameSuggestion).toHaveBeenCalledWith(server, 'hangar-2');
    expect(calls.dismissed).toBe(1);
  });

  it('falha ao renomear aparece na faixa', async () => {
    vi.mocked(core.renameSession).mockRejectedValueOnce(new Error('nome em uso'));
    const { calls, apply, target } = render();
    apply.click();
    await flush();
    expect(target.querySelector('[role="alert"]')?.textContent).toBe('nome em uso');
    expect(calls.renamed).toEqual([]);
  });

  it('Outra força uma sugestão nova e a devolve', async () => {
    const { calls, again } = render();
    again.click();
    await flush();
    expect(core.requestRenameSuggestion).toHaveBeenCalledWith(server, 'hangar-2', true);
    expect(calls.suggested).toEqual([{ suggestion: 'etiquetas-por-maquina', weak: false }]);
  });

  it('só a sugestão fraca mostra o aviso de pouco contexto', () => {
    expect(render(true).target.querySelector('.rename-weak')).not.toBeNull();
  });

  it('sugestão firme não mostra o aviso', () => {
    expect(render().target.querySelector('.rename-weak')).toBeNull();
  });
});
