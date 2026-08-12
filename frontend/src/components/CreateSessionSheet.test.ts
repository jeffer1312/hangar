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

// Reabrir PARTINDO de fechado: depois de um create bem-sucedido o onClose derrubou o sheet, então
// o primeiro clique já abre (a função reabrir() acima alternaria duas vezes e voltaria a fechar).
async function reabrirFechado() {
  (document.querySelector('[data-testid="sheet-toggle"]') as HTMLElement).click();
  await flush();
  await escolherPasta();
}

// Catálogos DISJUNTOS por conta (B4): a conta ativa conhece sonnet; a .claude-nova só conhece
// gpt-novo. Se o catálogo da conta anterior sobreviver à troca, a opção errada fica clicável.
function mockModelosPorConta() {
  vi.mocked(api.modelOptions).mockImplementation(async (_p, _e, cfg) =>
    cfg === '/home/x/.claude-nova'
      ? { kind: 'claude', reduced: false, models: [{ id: 'gpt-novo' }] }
      : { kind: 'claude', reduced: true, models: [{ id: 'opus' }, { id: 'sonnet' }, { id: 'haiku' }] },
  );
}

async function confirmarApagar() {
  [...(document.querySelectorAll('.conta-add') as unknown as HTMLElement[])]
    .find((b) => b.textContent === 'apagar')!.click();
  await tick();
  // Com confirmandoApagar=true o botão da linha some — o único 'apagar' restante é o da
  // confirmação (o mesmo find volta a pegar o certo).
  [...(document.querySelectorAll('.conta-add') as unknown as HTMLElement[])]
    .find((b) => b.textContent === 'apagar')!.click();
  await flush();
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

    // A LISTA também não pode ser a do Pi: sem o `modelos = []` do reset, a lista antiga com
    // gpt-5.6-luna continuaria clicável dentro de uma sessão Claude (o combo abriria e o id do Pi
    // estaria lá pra ser escolhido).
    (document.querySelector('#model-pick') as HTMLElement).click();
    await tick();
    const rotulos = [...document.querySelectorAll('.sel-item')].map((b) => b.textContent ?? '');
    expect(rotulos.some((t) => t.includes('gpt-5.6-luna'))).toBe(false);
    (document.body.querySelector('.sel-fora') as HTMLElement)?.click();
    await tick();

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
    // lista de contas fora do ar — o combo tem opções além de "Padrão". E o aviso de lista
    // reduzida é uma live region (role=status), não um texto mudo.
    (document.querySelector('#model-pick') as HTMLElement).click();
    await tick();
    const rotulos = [...document.querySelectorAll('.sel-item')].map((b) => b.textContent);
    expect(rotulos.some((t) => t?.includes('opus'))).toBe(true);
    (document.body.querySelector('.sel-fora') as HTMLElement)?.click();
    await tick();
    const aviso = [...document.querySelectorAll('.model-hint')]
      .find((p) => p.textContent?.includes('Lista reduzida'));
    expect(aviso?.getAttribute('role')).toBe('status');
    unmount(comp);
  });

  it('C: rejeição de chamada SUPERADA não apaga a escolha feita depois (guarda de geração)', async () => {
    // 1ª chamada fica pendente (a do "servidor A", que depois cai); a 2ª resolve.
    let rejeitar1!: (e: Error) => void;
    let chamadas = 0;
    vi.mocked(api.listClaudeConfigs).mockImplementation(() => {
      chamadas++;
      if (chamadas === 1) {
        return new Promise((_res, rej) => { rejeitar1 = rej; });
      }
      return Promise.resolve([]);
    });

    const { comp } = montar();
    await flush();
    await escolherPasta();            // loadConfigs #1 em voo (pendente)
    await reabrir();                  // loadConfigs #2 resolve -> lista do Claude na tela
    await escolherNoCombo('#model-pick', 'sonnet');

    // A chamada #1, já superada, finalmente rejeita — com a guarda, é descartada.
    rejeitar1(new Error('servidor A caiu'));
    await flush();

    expect(document.querySelector('#model-pick')!.textContent).toContain('sonnet');
    (document.querySelector('.primary-btn') as HTMLElement).click();
    await flush();
    expect(onCreate).toHaveBeenCalledWith('x', '/tmp/x', null, 'claude', null, 'sonnet', null);
    unmount(comp);
  });

  it('D: nome acessível do esforço e roles dos avisos (WCAG 2.5.3 e 4.1.3)', async () => {
    vi.mocked(api.listClaudeConfigs).mockRejectedValue(new Error('fora do ar'));

    const { comp } = montar();
    await flush();
    await escolherPasta();

    // Claude: nome acessível "Esforço" (igual ao rótulo visível).
    expect(document.querySelector('#effort-pick')!.getAttribute('aria-label')).toBe('Esforço');

    // Pi: o rótulo visível vira "Raciocínio" e o nome acessível tem que acompanhar (Label in Name).
    [...(document.querySelectorAll('.provider-btn') as unknown as HTMLElement[])]
      .find((b) => b.textContent === 'Pi')!.click();
    await flush();
    expect(document.querySelector('#effort-pick')!.getAttribute('aria-label')).toBe('Raciocínio');

    // Erro de listagem: modelOptions rejeita -> aviso com role=alert (o mesmo padrão do aviso de
    // conta deste arquivo), anunciado em vez de mudo.
    vi.mocked(api.modelOptions).mockRejectedValueOnce(new Error('provedor fora do ar'));
    [...(document.querySelectorAll('.provider-btn') as unknown as HTMLElement[])]
      .find((b) => b.textContent === 'Claude')!.click();
    await flush();
    const erro = [...document.querySelectorAll('.model-hint')]
      .find((p) => p.textContent?.includes('sessão abre no padrão'));
    expect(erro?.getAttribute('role')).toBe('alert');

    // A mensagem de FALHA AO CRIAR também é anunciada (role=alert) — é o erro do fluxo principal
    // desta tela, e é a que nenhum teste cobria.
    vi.mocked(onCreate).mockRejectedValueOnce(new Error('falha ao criar'));
    (document.querySelector('.primary-btn') as HTMLElement).click();
    await flush();
    expect(document.querySelector('.error-msg')?.getAttribute('role')).toBe('alert');
    unmount(comp);
  });
});

describe('CreateSessionSheet — B4/B6 da revisão final da branch', () => {
  it('B4: criar conta recarrega o catálogo — modelo da conta anterior não vaza pro create', async () => {
    vi.mocked(api.listClaudeConfigs).mockResolvedValue([
      { path: '/home/x/.claude', label: 'atual', active: true },
      { path: '/home/x/.claude-nova', label: '.claude-nova', active: false },
    ]);
    mockModelosPorConta();
    vi.mocked(api.criarConta).mockResolvedValue(
      { path: '/home/x/.claude-nova', label: '.claude-nova', active: false });

    const { comp } = montar();
    await flush();
    await escolherPasta();
    await escolherNoCombo('#model-pick', 'sonnet');   // escolha na conta A

    // "+ conta": caminho programático que não recarregava o catálogo (B4).
    (document.querySelector('.conta-row .conta-add') as HTMLElement).click();
    await tick();
    const input = document.querySelector('.conta-nova input') as HTMLInputElement;
    input.value = 'nova';
    input.dispatchEvent(new Event('input'));
    await tick();
    (document.querySelector('.conta-nova .conta-add') as HTMLElement).click(); // criar
    await flush();

    // A conta nova virou a seleção e a escolha da conta A foi zerada: combo em Padrão, e a
    // lista agora é a da conta B (disjunta) — 'sonnet' nem está clicável.
    expect((document.querySelector('#model-pick') as HTMLElement).textContent).toContain('Padrão');
    (document.querySelector('#model-pick') as HTMLElement).click();
    await tick();
    const rotulos = [...document.querySelectorAll('.sel-item')].map((b) => b.textContent ?? '');
    expect(rotulos.some((t) => t.includes('sonnet'))).toBe(false);
    expect(rotulos.some((t) => t.includes('gpt-novo'))).toBe(true);
    (document.body.querySelector('.sel-fora') as HTMLElement)?.click();
    await tick();

    // create manda model/effort NULOS até o usuário escolher na lista B.
    (document.querySelector('.primary-btn') as HTMLElement).click();
    await flush();
    expect(onCreate).toHaveBeenCalledWith(
      'x', '/tmp/x', '/home/x/.claude-nova', 'claude', null, null, null);
    vi.mocked(api.modelOptions).mockRestore();
    unmount(comp);
  });

  it('B4: apagar conta recarrega o catálogo — modelo da conta apagada não vaza pro create', async () => {
    vi.mocked(api.listClaudeConfigs)
      .mockResolvedValueOnce([
        { path: '/home/x/.claude', label: 'atual', active: true },
        { path: '/home/x/.claude-nova', label: '.claude-nova', active: false },
      ])
      .mockResolvedValue([{ path: '/home/x/.claude', label: 'atual', active: true }]);
    mockModelosPorConta();
    vi.mocked(api.apagarConta).mockResolvedValue(undefined);

    const { comp } = montar();
    await flush();
    await escolherPasta();
    // Seleciona a conta apagável: o onchange do combo já recarrega a lista dela (caminho vivo).
    await escolherNoCombo('#cfg-pick', '.claude-nova');
    await escolherNoCombo('#model-pick', 'gpt-novo');
    await confirmarApagar();

    // A seleção voltou pra conta ativa, a escolha da apagada foi zerada e a lista é a da ativa.
    expect((document.querySelector('#model-pick') as HTMLElement).textContent).toContain('Padrão');
    (document.querySelector('#model-pick') as HTMLElement).click();
    await tick();
    const rotulos = [...document.querySelectorAll('.sel-item')].map((b) => b.textContent ?? '');
    expect(rotulos.some((t) => t.includes('gpt-novo'))).toBe(false);
    expect(rotulos.some((t) => t.includes('sonnet'))).toBe(true);
    (document.body.querySelector('.sel-fora') as HTMLElement)?.click();
    await tick();

    (document.querySelector('.primary-btn') as HTMLElement).click();
    await flush();
    expect(onCreate).toHaveBeenCalledWith(
      'x', '/tmp/x', '/home/x/.claude', 'claude', null, null, null);
    vi.mocked(api.modelOptions).mockRestore();
    unmount(comp);
  });

  it('B4: apagar com a lista de contas fora do ar também recarrega o catálogo', async () => {
    vi.mocked(api.listClaudeConfigs)
      .mockResolvedValueOnce([
        { path: '/home/x/.claude', label: 'atual', active: true },
        { path: '/home/x/.claude-nova', label: '.claude-nova', active: false },
      ])
      .mockRejectedValue(new Error('fora do ar'));
    mockModelosPorConta();
    vi.mocked(api.apagarConta).mockResolvedValue(undefined);

    const { comp } = montar();
    await flush();
    await escolherPasta();
    await escolherNoCombo('#cfg-pick', '.claude-nova');
    await escolherNoCombo('#model-pick', 'gpt-novo');
    await confirmarApagar();

    // O aviso de refresh falho aparece, a seleção volta pra ativa e a escolha da apagada some.
    expect(document.body.textContent).toContain('não consegui atualizar a lista');
    expect((document.querySelector('#model-pick') as HTMLElement).textContent).toContain('Padrão');
    (document.querySelector('.primary-btn') as HTMLElement).click();
    await flush();
    expect(onCreate).toHaveBeenCalledWith(
      'x', '/tmp/x', '/home/x/.claude', 'claude', null, null, null);
    vi.mocked(api.modelOptions).mockRestore();
    unmount(comp);
  });

  it('B6: escolher Padrão apaga a preferência — a reabertura seguinte abre em Padrão', async () => {
    vi.mocked(api.listClaudeConfigs)
      .mockResolvedValue([{ path: '/home/x/.claude', label: 'atual', active: true }]);

    const { comp } = montar();
    await flush();

    // 1ª abertura: escolhe sonnet/high e cria — a memória é gravada.
    await escolherPasta();
    await escolherNoCombo('#model-pick', 'sonnet');
    await escolherNoCombo('#effort-pick', 'high');
    (document.querySelector('.primary-btn') as HTMLElement).click();
    await flush();
    expect(localStorage.getItem('cp_last_model::claude:-')).toBe('sonnet');
    expect(localStorage.getItem('cp_last_model::claude:-:effort')).toBe('high');

    // 2ª abertura: a memória restaura sonnet/high; o usuário escolhe Padrão nos dois e cria.
    await reabrirFechado();
    expect((document.querySelector('#model-pick') as HTMLElement).textContent).toContain('sonnet');
    await escolherNoCombo('#model-pick', 'Padrão');
    await escolherNoCombo('#effort-pick', 'Padrão');
    (document.querySelector('.primary-btn') as HTMLElement).click();
    await flush();

    // B6: o create APAGA as chaves — sem o removeItem, a 3ª abertura restauraria sonnet/high.
    expect(localStorage.getItem('cp_last_model::claude:-')).toBeNull();
    expect(localStorage.getItem('cp_last_model::claude:-:effort')).toBeNull();

    // 3ª abertura: abre em Padrão, não na escolha da 1ª.
    await reabrirFechado();
    expect((document.querySelector('#model-pick') as HTMLElement).textContent).toContain('Padrão');
    expect((document.querySelector('#effort-pick') as HTMLElement).textContent).toContain('Padrão');
    unmount(comp);
  });
});
