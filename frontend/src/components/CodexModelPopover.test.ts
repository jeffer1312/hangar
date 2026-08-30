// @vitest-environment happy-dom
// As duas pills do Codex, que substituíram a folha (ticket 12). O que estes casos travam é a única
// lógica que a troca de formato NÃO herdou de graça: os níveis de esforço são POR MODELO (medido em
// 30/08/2026, codex-cli 0.151.0 — `gpt-5.6-sol` aceita `ultra`, `gpt-5.5` não aceita nem `max`), e o
// POST exige o modelo junto, então "trocar só o nível" tem que mandar o modelo atual de volta.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import CodexModelPopover from './CodexModelPopover.svelte';
import CodexEffortPopover from './CodexEffortPopover.svelte';
import * as api from '../lib/api';

const onApplied = vi.hoisted(() => vi.fn());
const onClose = vi.hoisted(() => vi.fn());

vi.mock('../lib/api', () => ({
  getCodexModels: vi.fn(),
  setCodexModel: vi.fn(),
}));

const apiMock = vi.mocked(api);

const CATALOGO = {
  models: [
    { model: 'gpt-5.6-sol', displayName: 'GPT-5.6-Sol', description: 'Frontier.',
      efforts: [{ value: 'low' }, { value: 'high' }, { value: 'ultra' }], defaultEffort: 'low' },
    { model: 'gpt-5.5', displayName: 'GPT-5.5', description: '',
      efforts: [{ value: 'low' }, { value: 'high' }, { value: 'xhigh' }], defaultEffort: 'medium' },
  ],
  current: { model: 'gpt-5.5', effort: 'xhigh' },
};

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) { await tick(); await new Promise((r) => setTimeout(r, 0)); }
}

// A âncora precisa existir: o Popover mede a pill pra se posicionar.
function montar(Componente: unknown) {
  const el = document.createElement('div');
  const pill = document.createElement('button');
  document.body.append(el, pill);
  const comp = mount(Componente as never, {
    target: el,
    props: { open: true, anchor: pill, sessionName: 'x', onApplied, onClose },
  });
  return { comp };
}

function linhas(): HTMLButtonElement[] {
  return [...document.querySelectorAll('.linha')] as HTMLButtonElement[];
}

beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = '';
  apiMock.getCodexModels.mockResolvedValue(CATALOGO as never);
  apiMock.setCodexModel.mockResolvedValue(undefined as never);
});

describe('CodexModelPopover', () => {
  it('marca o modelo atual, que vem do próprio GET', async () => {
    const { comp } = montar(CodexModelPopover);
    await flush();
    const ativa = linhas().filter((b) => b.classList.contains('ativa'));
    expect(ativa.map((b) => b.textContent?.trim())).toEqual(['GPT-5.5']);
    unmount(comp);
  });

  it('trocar de modelo leva o esforço padrão DO NOVO, não o antigo', async () => {
    // `xhigh` está escolhido no gpt-5.5 e o gpt-5.6-sol não o lista: carregá-lo junto pediria um
    // nível que aquele modelo não tem. É o que o `pickModel` da folha antiga já fazia.
    const { comp } = montar(CodexModelPopover);
    await flush();
    linhas()[0].click();
    await flush();
    expect(apiMock.setCodexModel).toHaveBeenCalledWith('x', 'gpt-5.6-sol', 'low');
    expect(onApplied).toHaveBeenCalledWith('gpt-5.6-sol', 'low');
    unmount(comp);
  });

  it('reescolher o MESMO modelo preserva o esforço em uso', async () => {
    // Sem isto, tocar na linha já marcada rebaixaria `xhigh` pro default do modelo — uma troca que
    // ninguém pediu, disparada por um clique que não devia mudar nada.
    const { comp } = montar(CodexModelPopover);
    await flush();
    linhas()[1].click();
    await flush();
    expect(apiMock.setCodexModel).toHaveBeenCalledWith('x', 'gpt-5.5', 'xhigh');
    unmount(comp);
  });

  it('falha ao aplicar mantém o popover aberto e não avisa sucesso', async () => {
    apiMock.setCodexModel.mockRejectedValue(new Error('boom'));
    const { comp } = montar(CodexModelPopover);
    await flush();
    linhas()[0].click();
    await flush();
    expect(onApplied).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(document.querySelector('.err')?.textContent).toContain('boom');
    unmount(comp);
  });
});

describe('CodexEffortPopover', () => {
  it('lista os níveis DO MODELO ATUAL, não uma lista fixa', async () => {
    const { comp } = montar(CodexEffortPopover);
    await flush();
    expect(linhas().map((b) => b.textContent?.trim())).toEqual(['low', 'high', 'xhigh']);
    unmount(comp);
  });

  it('trocar o nível manda o modelo atual junto', async () => {
    // `CodexModelBody.model` é obrigatório no backend: "troque só o nível" é "mantenha o modelo".
    const { comp } = montar(CodexEffortPopover);
    await flush();
    linhas()[0].click();
    await flush();
    expect(apiMock.setCodexModel).toHaveBeenCalledWith('x', 'gpt-5.5', 'low');
    expect(onApplied).toHaveBeenCalledWith('low');
    unmount(comp);
  });

  it('sem modelo conhecido não oferece nível nenhum, mesmo com catálogo cheio', async () => {
    // O caso real: sessão sem client vivo e sem sidecar devolve `current.model: null` COM o
    // catálogo inteiro. Cair no primeiro modelo da lista faria um toque aqui trocar o MODELO sem
    // ninguém pedir — e calado, porque a pill do lado seguiria dizendo "Modelo". Quem resolve esse
    // caso é a pill de modelo.
    apiMock.getCodexModels.mockResolvedValue(
      { models: CATALOGO.models, current: { model: null, effort: null } } as never);
    const { comp } = montar(CodexEffortPopover);
    await flush();
    expect(linhas()).toHaveLength(0);
    expect(apiMock.setCodexModel).not.toHaveBeenCalled();
    unmount(comp);
  });

  it('mostra a descrição que o provedor manda pra cada nível', async () => {
    // A folha antiga mostrava, e o backend envia (`supportedReasoningEfforts[].description`).
    apiMock.getCodexModels.mockResolvedValue({
      models: [{ model: 'gpt-5.5', displayName: 'GPT-5.5', efforts: [
        { value: 'low', description: 'Fast responses with lighter reasoning' }] }],
      current: { model: 'gpt-5.5', effort: 'low' },
    } as never);
    const { comp } = montar(CodexEffortPopover);
    await flush();
    expect(document.querySelector('.meta')?.textContent).toBe('Fast responses with lighter reasoning');
    unmount(comp);
  });
});
