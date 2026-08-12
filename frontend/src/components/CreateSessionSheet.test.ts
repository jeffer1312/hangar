// @vitest-environment happy-dom
// Bloqueador da round 1 da Task 5 (parecer task5-a341c12): a escolha de modelo/esforço de uma
// abertura sobrevive à reabertura quando o /api/claude-configs falha. Duas metades protegem
// cenários distintos, e o teste cobre as duas:
//  A) fetch PENDENTE (host lento): nem .then nem .catch rodam — só o reset do $effect de abertura
//     zera os cinco campos novos. Sem ele, provider=claude + model=openai-codex/gpt-5.6-luna vai
//     pro create — pane sobe e o Claude reclama no primeiro turno, calado.
//  B) fetch REJEITANDO (404): o .catch precisa re-pedir a lista (carregarModelos) — senão a tela
//     fica com o combo só com "Padrão", mesmo com o backend vivo por trás.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick, createRawSnippet } from 'svelte';
// Harness, não o sheet: segura `open` e alterna por clique (o $set do mount é bloqueado em DEV no
// Svelte 5 — component_api_changed), reproduzindo o ciclo fechar/reabrir do app real.
import Harness from './CreateSessionSheet.harness.svelte';
import * as api from '../lib/api';

const onCreate = vi.hoisted(() => vi.fn(async () => {}));

vi.mock('../lib/api', () => ({
  listClaudeConfigs: vi.fn(),
  // Devolve a lista certa por provider: pro Claude os aliases (como o backend real), pro Pi o
  // modelo fake — a memória do Pi jamais pode casar com a lista do Claude.
  modelOptions: vi.fn(async (provider: string) =>
    provider === 'pi'
      ? { kind: 'pi', reduced: false,
          models: [{ provider: 'openai-codex', id: 'gpt-5.6-luna', context: '272K', images: true }] }
      : { kind: 'claude', reduced: true, models: [{ id: 'opus' }, { id: 'sonnet' }, { id: 'haiku' }] }),
  getSessions: vi.fn(async () => []),
  getEngines: vi.fn(async () => ({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '' })),
  criarConta: vi.fn(), apagarConta: vi.fn(),
}));
vi.mock('./FolderScanner.svelte', () => ({
  default: createRawSnippet(() => ({ render: () => '<div />' })),
}));

function flush(): Promise<void> {
  // Microtasks do fetch mockado + um tick de render do Svelte, várias vezes: o loadConfigs dispara
  // promises encadeadas (.then/.catch -> carregarModelos -> modelOptions).
  return (async () => {
    for (let i = 0; i < 5; i++) { await tick(); await new Promise((r) => setTimeout(r, 0)); }
  })();
}

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(Harness, {
    target: el,
    props: { onCreate, onOpenSession: vi.fn() },
  });
  return { el, comp };
}

// Leva a folha até o passo 2 (pasta escolhida) pelo caminho manual — o scanner é stub. O conteúdo
// do sheet vive num PORTAL pro document.body (BottomSheet), então as buscas usam `document`.
async function escolherPasta() {
  (document.querySelector('.advanced-toggle') as HTMLElement).click();
  await tick();
  const input = document.querySelector('.manual-form input') as HTMLInputElement;
  input.value = '/tmp/x';
  input.dispatchEvent(new Event('input'));
  await tick();
  const form = document.querySelector('.manual-form') as HTMLFormElement;
  form.dispatchEvent(new Event('submit', { cancelable: true }));
  await flush();
}

async function escolherNoCombo(sel: string, texto: string) {
  (document.querySelector(sel) as HTMLElement).click();
  await tick();
  const itens = [...document.querySelectorAll('.sel-item')] as HTMLElement[];
  const alvo = itens.find((b) => b.textContent?.includes(texto));
  if (!alvo) throw new Error(`opção "${texto}" não encontrada no combo ${sel}`);
  alvo.click();
  await tick();
}

// Suja o estado com a escolha de "abertura anterior": Pi + openai-codex/gpt-5.6-luna + high.
async function escolhaAnterior() {
  [...(document.querySelectorAll('.provider-btn') as unknown as HTMLElement[])]
    .find((b) => b.textContent === 'Pi')!.click();
  await flush();
  await escolherNoCombo('#model-pick', 'gpt-5.6-luna');
  await escolherNoCombo('#effort-pick', 'high');
}

function reabrir() {
  (document.querySelector('[data-testid="sheet-toggle"]') as HTMLElement).click();
  // Sem o flush entre os dois cliques o harness alterna duas vezes e volta ao estado inicial.
  return (async () => {
    await flush();
    (document.querySelector('[data-testid="sheet-toggle"]') as HTMLElement).click();
    await flush();
    // O reset de abertura volta ao passo 1 (pasta), como no app real.
    await escolherPasta();
  })();
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  document.body.innerHTML = '';
  // Default: fetch de contas PENDENTE (nunca resolve) — o cenário A.
  vi.mocked(api.listClaudeConfigs).mockImplementation(() => new Promise(() => {}));
});

describe('CreateSessionSheet — reabertura com a lista de contas fora do ar', () => {
  it('A: fetch pendente — a escolha da abertura anterior não sobrevive ao reabrir', async () => {
    // Memória simulada de uma escolha feita (o create a gravaria): a chave é do Pi, e a lista do
    // Claude não pode casá-la — vira Padrão, nunca flag.
    localStorage.setItem('cp_last_model::pi:-', 'openai-codex/gpt-5.6-luna');
    localStorage.setItem('cp_last_model::pi:-:effort', 'high');

    const { comp } = montar();
    await flush();
    await escolherPasta();
    await escolhaAnterior();
    await reabrir();

    // O $effect de abertura zerou os cinco campos (nenhum fetch resolveu pra re-pedir a lista).
    expect(document.querySelector('#model-pick')!.textContent).toContain('Padrão');
    expect(document.querySelector('#effort-pick')!.textContent).toContain('Padrão');

    // O create manda provider=claude com model/effort NULOS — o cenário do bloqueador morre aqui.
    (document.querySelector('.primary-btn') as HTMLElement).click();
    await flush();
    expect(onCreate).toHaveBeenCalledWith('x', '/tmp/x', null, 'claude', null, null, null);
    unmount(comp);
  });

  it('B: fetch rejeitando — o .catch re-pede a lista (a tela não fica sem modelos)', async () => {
    vi.mocked(api.listClaudeConfigs).mockRejectedValue(new Error('fora do ar'));

    const { comp } = montar();
    await flush();
    await escolherPasta();

    // Com o .catch chamando carregarModelos, a lista do Claude (aliases) carrega mesmo com a
    // lista de contas fora do ar — o combo tem opções além de "Padrão".
    (document.querySelector('#model-pick') as HTMLElement).click();
    await tick();
    const rotulos = [...document.querySelectorAll('.sel-item')].map((b) => b.textContent);
    expect(rotulos.some((t) => t?.includes('opus'))).toBe(true);
    unmount(comp);
  });
});
