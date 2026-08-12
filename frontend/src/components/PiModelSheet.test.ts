// @vitest-environment happy-dom
// Bloqueador 1 da round 1 da Task 6 (parecer task6-6caac2f): a folha oferecia modelos que o
// Aplicar recusa — a lista passou a ser o catálogo do `pi --list-models` (390), mas quem valida
// é o `pi_models.check_known` contra o catálogo DO SIDECAR (388; medido em 12/08/2026: 2 modelos
// só na lista nova, entre eles openrouter/bytedance-seed/seed-2.0-code) — e o cast `as PiModel[]`
// escondia que os objetos do catálogo não satisfazem o tipo. O desenho novo: SIDECAR é o
// conjunto, catálogo é só o enriquecimento (contexto/👁).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import PiModelSheet from './PiModelSheet.svelte';
import * as api from '../lib/api';

const onApplied = vi.hoisted(() => vi.fn());
const onClose = vi.hoisted(() => vi.fn());

vi.mock('../lib/api', () => ({
  getPiModels: vi.fn(),
  setPiModel: vi.fn(),
  modelOptions: vi.fn(),
}));

const apiMock = vi.mocked(api);

const SIDECAR = {
  models: [
    { provider: 'kimi-coding', id: 'k3', name: 'Kimi K3', reasoning: true },
    { provider: 'kimi-coding', id: 'k3-256k', name: 'Kimi K3-256K', reasoning: true },
  ],
  current: { provider: 'kimi-coding', id: 'k3', name: 'Kimi K3' },
  thinking: 'max',
  levels: ['low', 'high', 'max'],
};

const CATALOGO = {
  kind: 'pi', reduced: false,
  models: [
    { provider: 'kimi-coding', id: 'k3', context: '1.0M', images: true },
    { provider: 'kimi-coding', id: 'k3-256k', context: '262.1K', images: false },
    // Só no catálogo — o 422 medido na round 1 (openrouter/bytedance-seed/seed-2.0-code).
    { provider: 'openrouter', id: 'bytedance-seed/seed-2.0-code', context: '1.0M', images: false },
  ],
};

async function flush(): Promise<void> {
  // Microtasks do fetch mockado + um tick de render do Svelte, várias vezes: o load encadeia
  // allSettled -> map -> atribuição de estado.
  for (let i = 0; i < 5; i++) { await tick(); await new Promise((r) => setTimeout(r, 0)); }
}

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(PiModelSheet, {
    target: el,
    props: { open: true, sessionName: 'x', onApplied, onClose },
  });
  return { el, comp };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('PiModelSheet — sidecar é o conjunto, catálogo é o enriquecimento', () => {
  it('modelo que só existe no catálogo não aparece; quem casa ganha etiqueta com espaço', async () => {
    apiMock.getPiModels.mockResolvedValue(SIDECAR);
    apiMock.modelOptions.mockResolvedValue(CATALOGO);
    const { comp } = montar();
    await flush();
    const linhas = document.querySelectorAll('.model-row');
    // O conteúdo do sheet vive num PORTAL pro document.body (BottomSheet).
    expect(linhas.length).toBe(2);
    expect(document.body.textContent).not.toContain('bytedance-seed');
    const k3 = [...linhas].find((b) => b.textContent!.includes('Kimi K3'))!;
    expect(k3.textContent).toContain('Kimi K3');          // título = nome comercial do sidecar
    expect(k3.textContent).toContain('k3 · 1.0M · 👁');   // meta = id · contexto · 👁, com espaço
    expect(k3.getAttribute('aria-pressed')).toBe('true'); // current resolvido contra a lista
    const k3256 = [...linhas].find((b) => b.textContent!.includes('Kimi K3-256K'))!;
    expect(k3256.textContent).toContain('k3-256k · 262.1K');
    expect(k3256.textContent).not.toContain('👁');
    unmount(comp);
  });

  it('catálogo falhou: lista do sidecar sem etiqueta, níveis e thinking intactos', async () => {
    apiMock.getPiModels.mockResolvedValue(SIDECAR);
    apiMock.modelOptions.mockRejectedValue(new Error('pi --list-models falhou'));
    const { comp } = montar();
    await flush();
    const linhas = document.querySelectorAll('.model-row');
    expect(linhas.length).toBe(2);
    for (const b of linhas) expect(b.textContent).not.toContain('·'); // sem etiqueta
    // Bloco de níveis segue vivo (só o sidecar o alimenta) e o thinking do sidecar é o pressionado.
    expect(document.querySelector('.section-label')?.textContent).toContain('Nível de raciocínio');
    const ativo = document.querySelector('.effort-row[aria-pressed="true"]');
    expect(ativo?.textContent).toContain('max');
    unmount(comp);
  });
});
