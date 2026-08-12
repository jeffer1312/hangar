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
    { id: 'opus[1m]', name: 'Opus (1M context)', desc: 'Opus 5 with 1M context', active: true },
  ],
};

// Como era ANTES do backend dar id único: as duas linhas `opus` chegavam com o mesmo id.
const PICKER_ID_REPETIDO = {
  ...PICKER,
  models: PICKER.models.map((m) => (m.id === 'opus[1m]' ? { ...m, id: 'opus' } : m)),
};

async function flush(): Promise<void> {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0));
}

function montar(currentModel = 'Opus 5') {
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
      currentModel,
      currentEffort: 'high',
      onApply: vi.fn(),
      onClose: vi.fn(),
    },
  });
}

beforeEach(() => vi.clearAllMocks());

describe('ClaudeModelPopover — picker com keyword repetida', () => {
  it('desenha as seis linhas mesmo com o id repetido de antes', async () => {
    apiMock.getModelOptions.mockResolvedValue(PICKER_ID_REPETIDO);
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
    apiMock.getModelOptions.mockResolvedValue(PICKER_ID_REPETIDO);
    const comp = montar();
    await flush();
    expect(document.querySelector('.pop')?.textContent).not.toContain('Carregando');
    unmount(comp);
  });

  // Quem escolhe entre as duas linhas `opus` é a statusline citar a janela de 1M ou não. Sem isso,
  // a sessão rodando no 1M mostrava o tique no Opus normal — visto na tela.
  it('sessão no 1M marca a linha de 1M', async () => {
    apiMock.getModelOptions.mockResolvedValue(PICKER);
    const comp = montar('Opus5·1M');
    await flush();
    const marcada = document.querySelector('.pop .linha.ativa .nome')?.textContent?.trim();
    expect(marcada).toBe('Opus (1M context)');
    unmount(comp);
  });

  it('sessão no Opus normal marca a linha normal', async () => {
    apiMock.getModelOptions.mockResolvedValue(PICKER);
    const comp = montar('Opus 5');
    await flush();
    const marcada = document.querySelector('.pop .linha.ativa .nome')?.textContent?.trim();
    expect(marcada).toBe('Opus');
    unmount(comp);
  });
});
