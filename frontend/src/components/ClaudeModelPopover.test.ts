// @vitest-environment happy-dom
// Medido na tela em 12/08/2026, depois que as duas revisões finais aprovaram: a caixa do Claude
// ficava presa em "Carregando…" e o console trazia `each_key_duplicate`. Causa: o picker do Claude
// devolve DUAS linhas com a keyword `opus` — "Opus" e "Opus (1M context)" —, e `{#each ... (m.id)}`
// com chave repetida derruba o render inteiro no Svelte 5. Nenhuma suíte pegou porque nenhuma
// montava a lista com id repetido, e nenhum revisor abriu o app.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount } from 'svelte';
import ClaudeModelPopover from './ClaudeModelPopover.svelte';
import * as api from '../lib/api';

vi.mock('../lib/api', () => ({
  getModelOptions: vi.fn(),
  setEngineModel: vi.fn(),
}));

const apiMock = vi.mocked(api);

// Resposta real da máquina do usuário (GET /api/sessions/<n>/model/options).
const PICKER = {
  kind: 'claude' as const,
  engine: null,
  effort: 'high',
  models: [
    { id: 'default', name: 'Default', desc: 'Sonnet 5 · Efficient for routine tasks', active: false },
    { id: 'sonnet', name: 'Sonnet', desc: 'Sonnet 5 · Efficient for routine tasks', active: false },
    { id: 'fable', name: 'Fable', desc: 'Fable 5 · Most capable', active: false },
    { id: 'opus', name: 'Opus', desc: 'Opus 5 · Best for everyday, complex tasks', active: false },
    { id: 'haiku', name: 'Haiku', desc: 'Haiku 4.5 · Fastest', active: false },
    { id: 'opus', name: 'Opus (1M context)', desc: 'Opus 5 with 1M context', active: true },
  ],
};

async function flush(): Promise<void> {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0));
}

function montar() {
  document.body.innerHTML = '';
  const pill = document.createElement('button');
  const alvo = document.createElement('div');
  document.body.append(pill, alvo);
  return mount(ClaudeModelPopover, {
    target: alvo,
    props: {
      open: true,
      anchor: pill,
      sessionName: 'x',
      currentModel: 'Opus 5',
      currentEffort: 'high',
      onApply: vi.fn(),
      onClose: vi.fn(),
    },
  });
}

beforeEach(() => vi.clearAllMocks());

describe('ClaudeModelPopover — picker com keyword repetida', () => {
  it('desenha as seis linhas mesmo com dois `opus`', async () => {
    apiMock.getModelOptions.mockResolvedValue(PICKER);
    const comp = montar();
    await flush();
    const linhas = document.querySelectorAll('.pop .linha');
    expect(linhas.length).toBe(6);
    const nomes = [...linhas].map((b) => b.querySelector('.nome')?.textContent?.trim());
    expect(nomes).toContain('Opus');
    expect(nomes).toContain('Opus (1M context)');
    unmount(comp);
  });

  it('a lista não fica presa em "Carregando…"', async () => {
    apiMock.getModelOptions.mockResolvedValue(PICKER);
    const comp = montar();
    await flush();
    expect(document.querySelector('.pop')?.textContent).not.toContain('Carregando');
    unmount(comp);
  });
});
