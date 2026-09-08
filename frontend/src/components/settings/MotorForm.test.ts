// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import MotorForm from './MotorForm.svelte';
import * as apiLib from '@hangar/core';
import * as credLib from '../../lib/credenciais';
import * as m from '../../paraglide/messages';
import type { Motor } from '@hangar/core';

// O core traz muito mais que estas quatro funções; sem espalhar o original o mock derruba o resto
// que o componente importa de lá.
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  putEngine: vi.fn(async () => ({ motores: {} })),
  putEngineForServer: vi.fn(async () => ({ motores: {} })),
  engineModelos: vi.fn(async () => ({ modelos: [] })),
  engineModelosForServer: vi.fn(async () => ({ modelos: [] })),
}));
vi.mock('../../lib/credenciais', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/credenciais')>();
  return { ...real, sincronizarNosAgentes: vi.fn(async () => ({ resultado: { pi: { ok: true, motivo: '' } } })) };
});

const apiMock = vi.mocked(apiLib);
const credMock = vi.mocked(credLib);

const KIMI: Motor = {
  label: 'Kimi', base_url: 'https://api.kimi.com/coding', model: 'kimi-k3',
  context_window: 256000, vision: false, api_key: 'sk-k••••4f2a', api_key_definida: true,
  prompt_caching: false,
};

function montar(motor: Motor = KIMI) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const onSalvo = vi.fn();
  const onFechar = vi.fn();
  const comp = mount(MotorForm, { target: el, props: { apiTarget: null, nome: 'kimi', motor, onSalvo, onFechar } });
  return { el, comp: comp as never, onSalvo, onFechar };
}

// Modo criar: sem motor no disco e com o nome curto ainda por digitar.
function montarCriando(nomesExistentes: string[] = []) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const onSalvo = vi.fn();
  const onFechar = vi.fn();
  const comp = mount(MotorForm, {
    target: el,
    props: { apiTarget: null, nome: '', motor: null, criando: true, nomesExistentes, onSalvo, onFechar },
  });
  return { el, comp: comp as never, onSalvo, onFechar };
}
// happy-dom não faz layout, então `getBoundingClientRect` devolve 0 e quem informa a largura é o
// ResizeObserver: o que estes casos provam é a DECISÃO (estreito × largo), não o pixel. O stub
// precisa estar no lugar ANTES do mount e ser restaurado no fim — vazá-lo deixaria toda a suíte
// em modo estreito, calada. Precedente do stub: ActivitySheet.agente.test.ts.
async function comLargura<T>(w: number, montarFn: () => T) {
  const real = globalThis.ResizeObserver;
  let cb: ResizeObserverCallback | null = null;
  class RO {
    constructor(c: ResizeObserverCallback) { cb = c; }
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = RO;
  try {
    const t = montarFn();
    await espera();
    const disparar = async (largura: number) => {
      cb?.([{ contentRect: { width: largura } } as ResizeObserverEntry], null as never);
      await espera();
    };
    await disparar(w);
    return Object.assign(t as object, { disparar }) as T & { disparar: (l: number) => Promise<void> };
  } finally {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = real;
  }
}

const avancado = (el: HTMLElement) => el.querySelector<HTMLDetailsElement>('details.avancado')!;
// Casa pela ESTRUTURA, não pelo texto do rótulo: o nome de cada "?" carrega o campo, e comparar
// com um texto fixo esconderia justamente quatro botões chamados igual.
const perguntas = (el: HTMLElement) =>
  [...el.querySelectorAll<HTMLElement>('details.ajuda-q > summary')];
const acoes = (el: HTMLElement) => el.querySelector<HTMLElement>('.acoes')!;
const campo = (el: HTMLElement, name: string) => el.querySelector<HTMLInputElement>(`input[name="${name}"]`)!;
const botao = (el: HTMLElement, rotulo: string) =>
  [...el.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === rotulo)!;

const espera = async () => { for (let i = 0; i < 5; i++) await tick(); };

beforeEach(() => vi.clearAllMocks());

describe('MotorForm', () => {
  it('nasce com o motor lido: modelo, janela, chave definida e campo de chave VAZIO', () => {
    const t = montar();
    expect(campo(t.el, 'model').value).toBe('kimi-k3');
    expect(campo(t.el, 'context_window').value).toBe('256000');
    expect(t.el.textContent).toContain(m.config_motores_chave_definida());
    expect(campo(t.el, 'api_key').value).toBe('');
    expect(campo(t.el, 'api_key').placeholder).toBe(m.config_motores_colar_nova());
    unmount(t.comp);
  });

  // Os dez controles do Avançado são alcançáveis sem clique: escondê-los atrás do summary foi o
  // que fez a tela fundida entregar menos do que a tela Motores tinha.
  it('o avançado nasce ABERTO nos dois modos', () => {
    const t = montar();
    const det = t.el.querySelector<HTMLDetailsElement>('details');
    expect(det).not.toBeNull();
    expect(det!.open).toBe(true);
    unmount(t.comp);

    const c = montarCriando();
    expect(c.el.querySelector<HTMLDetailsElement>('details')!.open).toBe(true);
    unmount(c.comp);
  });

  it('só o modo criar tem o campo de nome curto, com a linha do claude-engine', () => {
    const c = montarCriando();
    expect(campo(c.el, 'nome')).toBeDefined();
    expect(c.el.textContent).toContain(m.config_motores_nome_curto());
    expect(c.el.textContent).toContain(m.config_motores_terminal_1());
    expect(c.el.textContent).toContain(m.config_motores_terminal_2());
    const codigos = [...c.el.querySelectorAll('code')].map((x) => x.textContent ?? '');
    expect(codigos.some((x) => x.startsWith('claude-engine'))).toBe(true);
    unmount(c.comp);

    // Em edição o nome é o id no disco e não muda — campo nenhum.
    const t = montar();
    expect(t.el.querySelector('input[name="nome"]')).toBeNull();
    unmount(t.comp);
  });

  // A ajuda é lida como um comando de verdade: com o campo vazio ela tem que mostrar o EXEMPLO do
  // placeholder, não o `chave` que o fallback do idDe('') produz — esse nome ninguém escolheu.
  it('com o nome curto vazio a ajuda mostra o exemplo, não o fallback do idDe', async () => {
    const c = montarCriando();
    const codigo = () => [...c.el.querySelectorAll('code')]
      .map((x) => x.textContent ?? '').find((x) => x.startsWith('claude-engine'))!;
    expect(codigo()).toBe('claude-engine kimi');

    const inp = campo(c.el, 'nome');
    inp.value = 'Meu Provedor';
    inp.dispatchEvent(new Event('input', { bubbles: true }));
    await espera();
    expect(codigo()).toBe('claude-engine meu-provedor');
    unmount(c.comp);
  });

  it('os atalhos de endereço preenchem o campo base_url', async () => {
    const t = montar();
    const atalho = botao(t.el, m.config_motores_dica_omni());
    expect(botao(t.el, m.config_motores_dica_kimi())).toBeDefined();
    expect(atalho).toBeDefined();
    atalho.click();
    await tick();
    expect(campo(t.el, 'base_url').value).toBe('https://ai.omniwise.com.br');
    unmount(t.comp);
  });

  // Também na criação: o bloco continua aberto depois do primeiro Salvar, e daí em diante o motor
  // existe e cada Salvar seguinte é edição.
  it('o formulário avisa nos dois modos que salvar não mexe em sessão aberta', () => {
    const t = montar();
    expect(t.el.textContent).toContain(m.config_motores_sessoes_abertas());
    unmount(t.comp);

    const c = montarCriando();
    expect(c.el.textContent).toContain(m.config_motores_sessoes_abertas());
    unmount(c.comp);
  });

  // O aviso fala de uma chave e um endereço JÁ salvos: em criação não há nenhum dos dois, e a
  // frase acendia no primeiro caractere digitado no endereço.
  it('o aviso do endereço editado é só de edição com chave salva', async () => {
    const c = montarCriando();
    const novo = campo(c.el, 'base_url');
    novo.value = 'https://x.exemplo';
    novo.dispatchEvent(new Event('input', { bubbles: true }));
    await espera();
    expect(c.el.textContent).not.toContain(m.config_motores_endereco_mudou());
    unmount(c.comp);

    const t = montar();
    const endereco = campo(t.el, 'base_url');
    endereco.value = 'https://outro.exemplo.com';
    endereco.dispatchEvent(new Event('input', { bubbles: true }));
    await espera();
    expect(t.el.textContent).toContain(m.config_motores_endereco_mudou());
    unmount(t.comp);
  });

  it('em criação o PUT vai no id derivado do nome curto, e sem nome não dá pra salvar', async () => {
    const c = montarCriando();
    expect(botao(c.el, m.ctx_salvar()).disabled).toBe(true);

    for (const [nome, valor] of [['nome', 'Meu Provedor'], ['base_url', 'https://x.exemplo'], ['model', 'mx-1']]) {
      const inp = campo(c.el, nome);
      inp.value = valor;
      inp.dispatchEvent(new Event('input', { bubbles: true }));
    }
    await espera();
    expect(botao(c.el, m.ctx_salvar()).disabled).toBe(false);

    botao(c.el, m.ctx_salvar()).click();
    await espera();
    const [id, corpo] = apiMock.putEngine.mock.calls[0];
    expect(id).toBe('meu-provedor');
    // Sem rótulo digitado, o nome curto é o rótulo.
    expect(corpo).toMatchObject({ label: 'Meu Provedor', base_url: 'https://x.exemplo', model: 'mx-1' });
    // Defaults do Avançado num motor que ainda não existe.
    expect(corpo).toMatchObject({ prompt_caching: true, adaptive_thinking: true, bundled_skills: false });
    expect(credMock.sincronizarNosAgentes).toHaveBeenCalledWith(null, 'chave:meu-provedor');
    unmount(c.comp);
  });

  it('salvar manda o registro COMPLETO (avançado incluso, vision preservada), chama onSalvo, sincroniza e NÃO fecha', async () => {
    apiMock.putEngine.mockResolvedValueOnce({ motores: { kimi: { ...KIMI, model: 'kimi-k3' } } });
    const t = montar();
    botao(t.el, m.ctx_salvar()).click();
    await tick(); await tick(); await tick();
    expect(apiMock.putEngine).toHaveBeenCalledTimes(1);
    const [nome, corpo] = apiMock.putEngine.mock.calls[0];
    expect(nome).toBe('kimi');
    expect(corpo).toMatchObject({
      label: 'Kimi', base_url: 'https://api.kimi.com/coding', model: 'kimi-k3', context_window: 256000,
      vision: false, prompt_caching: false, adaptive_thinking: true, bundled_skills: false,
    });
    expect(corpo).not.toHaveProperty('api_key');   // chave vazia = mantém a atual
    // 2º argumento: a máquina que RESPONDEU o PUT, capturada antes do await — é o que deixa o pai
    // escrever sob a chave certa mesmo depois de a tela ter trocado de alvo.
    expect(t.onSalvo).toHaveBeenCalledWith({ kimi: expect.objectContaining({ model: 'kimi-k3' }) }, null);
    expect(credMock.sincronizarNosAgentes).toHaveBeenCalledWith(null, 'chave:kimi');
    await tick();
    // O resultado da sincronização fica NA TELA: o bloco não fechou e o botão virou Fechar.
    expect(t.el.textContent).toContain(m.novacred_sync_titulo());
    expect(t.onFechar).not.toHaveBeenCalled();
    expect(botao(t.el, m.sessao_fechar())).toBeDefined();
    unmount(t.comp);
  });

  it('com alvo explícito, salva e sincroniza NAQUELE servidor', async () => {
    const alvo = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 't' };
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MotorForm, { target: el, props: { apiTarget: alvo, nome: 'kimi', motor: KIMI, onSalvo: vi.fn(), onFechar: vi.fn() } });
    botao(el, m.ctx_salvar()).click();
    await tick(); await tick(); await tick();
    expect(apiMock.putEngineForServer).toHaveBeenCalledWith(alvo, 'kimi', expect.any(Object));
    expect(apiMock.putEngine).not.toHaveBeenCalled();
    expect(credMock.sincronizarNosAgentes).toHaveBeenCalledWith(alvo, 'chave:kimi');
    unmount(comp);
  });

  it('endereço editado sem chave nova avisa que o Testar usaria o endereço antigo', async () => {
    const t = montar();
    const endereco = campo(t.el, 'base_url');
    endereco.value = 'https://outro.exemplo.com';
    endereco.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    expect(t.el.textContent).toContain(m.config_motores_endereco_mudou());
    unmount(t.comp);
  });

  it('motor SEM chave salvo sem chave nova continua sem chave definida', async () => {
    const semChave: Motor = { ...KIMI, api_key: '', api_key_definida: false };
    apiMock.putEngine.mockResolvedValueOnce({ motores: { kimi: semChave } });
    const t = montar(semChave);
    botao(t.el, m.ctx_salvar()).click();
    await tick(); await tick(); await tick();
    expect(apiMock.putEngine.mock.calls[0][1]).not.toHaveProperty('api_key');
    expect(t.el.textContent).not.toContain(m.config_motores_chave_definida());
    expect(campo(t.el, 'api_key').placeholder).toBe(m.config_motores_colar());
    unmount(t.comp);
  });

  it('salvar que falha mostra o erro, não chama onSalvo e não sincroniza', async () => {
    apiMock.putEngine.mockRejectedValueOnce(new Error('502 upstream'));
    const t = montar();
    botao(t.el, m.ctx_salvar()).click();
    await tick(); await tick(); await tick();
    expect(t.el.textContent).toContain('502 upstream');
    expect(t.onSalvo).not.toHaveBeenCalled();
    expect(credMock.sincronizarNosAgentes).not.toHaveBeenCalled();
    // O botão volta a funcionar: `salvando` preso em true deixaria a pessoa sem poder tentar de novo.
    expect(botao(t.el, m.ctx_salvar()).disabled).toBe(false);
    unmount(t.comp);
  });

  it('sincronização que falha NÃO desfaz o motor salvo — o erro aparece e o salvar continua valendo', async () => {
    credMock.sincronizarNosAgentes.mockRejectedValueOnce(new Error('pi fora do ar'));
    const t = montar();
    botao(t.el, m.ctx_salvar()).click();
    await tick(); await tick(); await tick(); await tick();
    expect(t.onSalvo).toHaveBeenCalledTimes(1);
    expect(t.el.textContent).toContain('pi fora do ar');
    expect(t.el.textContent).not.toContain(m.config_motores_erro_salvar());
    expect(botao(t.el, m.sessao_fechar())).toBeDefined();
    unmount(t.comp);
  });

  it('Testar traz os modelos do provedor e o modelo sem janela LIMPA o campo da janela', async () => {
    apiMock.engineModelos.mockResolvedValueOnce({
      modelos: [{ id: 'kimi-k3', context_length: null, vision: null }],
    });
    const t = montar();
    botao(t.el, m.config_motores_testar()).click();
    await tick(); await tick(); await tick();
    expect(apiMock.engineModelos).toHaveBeenCalledWith({ nome: 'kimi' });
    // 256000 veio do motor; o modelo escolhido não declara janela, então o número antigo sai —
    // manter estouraria a janela real do modelo novo.
    expect(campo(t.el, 'context_window').value).toBe('');
    unmount(t.comp);
  });

  it('salvar fica desabilitado sem modelo, e Testar sem endereço', async () => {
    const t = montar({ ...KIMI, model: '', base_url: '' });
    expect(botao(t.el, m.ctx_salvar()).disabled).toBe(true);
    expect(botao(t.el, m.config_motores_testar()).disabled).toBe(true);
    unmount(t.comp);
  });

  // O bloco fica aberto depois de salvar, então salvar duas vezes na mesma instância é o uso normal:
  // o que está na tela tem que ser o resultado da ÚLTIMA gravação.
  it('sync que falha no 2º salvar não deixa o "ok" da 1ª sincronização na tela', async () => {
    const t = montar();
    botao(t.el, m.ctx_salvar()).click();
    await espera();
    expect(t.el.textContent).toContain(m.novacred_sync_ok());

    credMock.sincronizarNosAgentes.mockRejectedValueOnce(new Error('pi fora do ar'));
    botao(t.el, m.ctx_salvar()).click();
    await espera();
    expect(t.el.textContent).toContain('pi fora do ar');
    expect(t.el.textContent).not.toContain(m.novacred_sync_ok());
    unmount(t.comp);
  });

  it('PUT que falha no 2º salvar não deixa o resultado da 1ª sincronização na tela', async () => {
    const t = montar();
    botao(t.el, m.ctx_salvar()).click();
    await espera();
    expect(t.el.textContent).toContain(m.novacred_sync_titulo());

    apiMock.putEngine.mockRejectedValueOnce(new Error('502 upstream'));
    botao(t.el, m.ctx_salvar()).click();
    await espera();
    expect(t.el.textContent).toContain('502 upstream');
    expect(t.el.textContent).not.toContain(m.novacred_sync_titulo());
    unmount(t.comp);
  });

  // Campo ausente do corpo herda o valor do disco (o `put_engine` do backend), e `null` também;
  // quem LIMPA é o campo presente e vazio. Por isso limpar na tela tem que MANDAR o campo: omitir
  // devolvia HTTP 200 com o valor antigo de volta, calado.
  it('limpar subagentes e janela manda os dois como "" — omitir faria o backend herdar os antigos', async () => {
    const t = montar({ ...KIMI, subagent_model: 'k2' });
    const sub = campo(t.el, 'subagent_model');
    expect(sub.value).toBe('k2');
    sub.value = '';
    sub.dispatchEvent(new Event('input', { bubbles: true }));
    const janela = campo(t.el, 'context_window');
    expect(janela.value).toBe('256000');
    janela.value = '';
    janela.dispatchEvent(new Event('input', { bubbles: true }));
    await espera();

    botao(t.el, m.ctx_salvar()).click();
    await espera();
    const corpo = apiMock.putEngine.mock.calls[0][1];
    expect(corpo).toHaveProperty('subagent_model', '');
    expect(corpo).toHaveProperty('context_window', '');
    unmount(t.comp);
  });

  // Uso comum: abre o motor, não toca em nada, salva. É o que prova que mandar o campo vazio SEMPRE
  // não estraga quem nunca preencheu opcional nenhum — `''` num campo que já estava vazio não
  // apaga nada no disco.
  it('motor sem opcionais salvo sem tocar em nada manda os quatro opcionais como ""', async () => {
    const t = montar({ ...KIMI, subagent_model: undefined, context_window: undefined });
    botao(t.el, m.ctx_salvar()).click();
    await espera();
    const corpo = apiMock.putEngine.mock.calls[0][1];
    expect(corpo).toHaveProperty('subagent_model', '');
    expect(corpo).toHaveProperty('context_window', '');
    expect(corpo).toHaveProperty('auto_compact_window', '');
    expect(corpo).toHaveProperty('max_output_tokens', '');
    unmount(t.comp);
  });

  // Criar não fecha o bloco (é onde o resultado da sincronização é lido), então a MESMA instância
  // salva de novo. Do 1º Salvar em diante o modelo existe no disco: o formulário é edição dele.
  describe('depois do primeiro Salvar em criação o formulário é edição', () => {
    const preencher = async (el: HTMLElement, pares: [string, string][]) => {
      for (const [nome, valor] of pares) {
        const inp = campo(el, nome);
        inp.value = valor;
        inp.dispatchEvent(new Event('input', { bubbles: true }));
      }
      await espera();
    };
    // O registro criado tem que voltar do PUT: com `{ motores: {} }` a reconciliação do
    // `api_key_definida` não acontece e o caso do aviso de endereço testaria o vazio.
    const criado = { motores: { kimi: { ...KIMI, api_key_definida: true } } };

    it('o 2º Salvar atualiza o mesmo id, mesmo com outro nome digitado — não cria um segundo modelo', async () => {
      apiMock.putEngine.mockResolvedValue(criado);
      const c = montarCriando();
      await preencher(c.el, [['nome', 'kimi'], ['base_url', 'https://api.kimi.com/coding'], ['model', 'kimi-k3']]);
      botao(c.el, m.ctx_salvar()).click();
      await espera();

      await preencher(c.el, [['nome', 'outro']]);
      botao(c.el, m.ctx_salvar()).click();
      await espera();

      expect(apiMock.putEngine).toHaveBeenCalledTimes(2);
      expect(apiMock.putEngine.mock.calls.map((x) => x[0])).toEqual(['kimi', 'kimi']);
      unmount(c.comp);
    });

    it('o campo do nome curto fica só-leitura depois do 1º Salvar', async () => {
      apiMock.putEngine.mockResolvedValue(criado);
      const c = montarCriando();
      await preencher(c.el, [['nome', 'kimi'], ['base_url', 'https://api.kimi.com/coding'], ['model', 'kimi-k3']]);
      expect(campo(c.el, 'nome').readOnly).toBe(false);

      botao(c.el, m.ctx_salvar()).click();
      await espera();
      expect(campo(c.el, 'nome').readOnly).toBe(true);
      unmount(c.comp);
    });

    // O aviso é de edição com chave salva; depois do 1º Salvar a criação virou exatamente isso, e
    // sem ele o Testar usaria a chave salva contra o endereço ANTIGO, calado.
    it('o aviso do endereço editado volta a acender depois do 1º Salvar', async () => {
      apiMock.putEngine.mockResolvedValue(criado);
      const c = montarCriando();
      await preencher(c.el, [
        ['nome', 'kimi'], ['base_url', 'https://api.kimi.com/coding'],
        ['model', 'kimi-k3'], ['api_key', 'sk-nova'],
      ]);
      // Antes de gravar não há endereço salvo: nada a avisar.
      await preencher(c.el, [['base_url', 'https://antes.exemplo']]);
      expect(c.el.textContent).not.toContain(m.config_motores_endereco_mudou());

      await preencher(c.el, [['base_url', 'https://api.kimi.com/coding']]);
      botao(c.el, m.ctx_salvar()).click();
      await espera();
      expect(c.el.textContent).not.toContain(m.config_motores_endereco_mudou());

      await preencher(c.el, [['base_url', 'https://depois.exemplo']]);
      expect(c.el.textContent).toContain(m.config_motores_endereco_mudou());
      unmount(c.comp);
    });

    it('o onSalvo do 2º Salvar traz o mesmo id, e a ajuda segue mostrando o comando do id gravado', async () => {
      apiMock.putEngine.mockResolvedValue(criado);
      const c = montarCriando();
      await preencher(c.el, [['nome', 'kimi'], ['base_url', 'https://api.kimi.com/coding'], ['model', 'kimi-k3']]);
      botao(c.el, m.ctx_salvar()).click();
      await espera();
      botao(c.el, m.ctx_salvar()).click();
      await espera();

      expect(c.onSalvo).toHaveBeenCalledTimes(2);
      expect(c.onSalvo.mock.calls[1][0]).toHaveProperty('kimi');
      const codigo = [...c.el.querySelectorAll('code')]
        .map((x) => x.textContent ?? '').find((x) => x.startsWith('claude-engine'));
      expect(codigo).toBe('claude-engine kimi');
      unmount(c.comp);
    });
  });

  // O PUT é substituição: criar com o nome curto de um modelo existente apagaria o registro dele.
  describe('nome curto já ocupado é recusado ANTES de gravar', () => {
    const preencher = async (el: HTMLElement, pares: [string, string][]) => {
      for (const [nome, valor] of pares) {
        const inp = campo(el, nome);
        inp.value = valor;
        inp.dispatchEvent(new Event('input', { bubbles: true }));
      }
      await espera();
    };

    it('criar com nome homônimo não chama o PUT nem a sincronização, e avisa', async () => {
      const c = montarCriando(['kimi']);
      // `Kimi` normaliza para `kimi` pelo idDe: a recusa é sobre o id, não sobre o que se digitou.
      await preencher(c.el, [['nome', 'Kimi'], ['base_url', 'https://x.exemplo'], ['model', 'mx-1']]);
      botao(c.el, m.ctx_salvar()).click();
      await espera();

      expect(apiMock.putEngine).not.toHaveBeenCalled();
      expect(credMock.sincronizarNosAgentes).not.toHaveBeenCalled();
      expect(c.el.textContent).toContain(m.config_motores_nome_em_uso());
      unmount(c.comp);
    });

    it('corrigir o nome no mesmo formulário grava normalmente e o aviso some', async () => {
      const c = montarCriando(['kimi']);
      await preencher(c.el, [['nome', 'kimi'], ['base_url', 'https://x.exemplo'], ['model', 'mx-1']]);
      botao(c.el, m.ctx_salvar()).click();
      await espera();
      expect(c.el.textContent).toContain(m.config_motores_nome_em_uso());

      await preencher(c.el, [['nome', 'kimi-2']]);
      botao(c.el, m.ctx_salvar()).click();
      await espera();
      expect(apiMock.putEngine).toHaveBeenCalledTimes(1);
      expect(apiMock.putEngine.mock.calls[0][0]).toBe('kimi-2');
      expect(c.el.textContent).not.toContain(m.config_motores_nome_em_uso());
      unmount(c.comp);
    });

    // A recusa vale só na criação: senão nenhum modelo do disco poderia ser salvo de novo.
    it('em edição o nome no disco está na lista e o salvar grava normalmente', async () => {
      const el = document.createElement('div');
      document.body.appendChild(el);
      const comp = mount(MotorForm, {
        target: el,
        props: { apiTarget: null, nome: 'kimi', motor: KIMI, nomesExistentes: ['kimi'], onSalvo: vi.fn(), onFechar: vi.fn() },
      });
      botao(el, m.ctx_salvar()).click();
      await espera();
      expect(apiMock.putEngine).toHaveBeenCalledTimes(1);
      expect(el.textContent).not.toContain(m.config_motores_nome_em_uso());
      unmount(comp);
    });

    it('o 2º Salvar do que acabou de ser criado não é recusado pelo próprio nome', async () => {
      apiMock.putEngine.mockResolvedValue({ motores: { kimi: { ...KIMI, api_key_definida: true } } });
      const lista: string[] = [];
      const c = montarCriando(lista);
      await preencher(c.el, [['nome', 'kimi'], ['base_url', 'https://api.kimi.com/coding'], ['model', 'kimi-k3']]);
      botao(c.el, m.ctx_salvar()).click();
      await espera();

      // A lista recarregada do pai já traz o modelo recém-criado.
      lista.push('kimi');
      botao(c.el, m.ctx_salvar()).click();
      await espera();

      expect(apiMock.putEngine).toHaveBeenCalledTimes(2);
      expect(c.el.textContent).not.toContain(m.config_motores_nome_em_uso());
      unmount(c.comp);
    });
  });

  // Quem decide "é estreito" é a largura do PRÓPRIO bloco, não a da janela: o mesmo formulário
  // mora na folha do celular e no card do desktop, e é o painel que aperta.
  describe('painel estreito', () => {
    it('no estreito o avançado nasce FECHADO e as ações grudam no pé', async () => {
      const c = await comLargura(390, () => montarCriando());
      expect(avancado(c.el).open).toBe(false);
      expect(acoes(c.el).classList.contains('fixo')).toBe(true);
      unmount(c.comp);

      const t = await comLargura(390, () => montar());
      expect(avancado(t.el).open).toBe(false);
      expect(acoes(t.el).classList.contains('fixo')).toBe(true);
      unmount(t.comp);
    });

    // Girar o telefone ou arrastar a largura do painel volta ao formulário inteiro.
    it('voltar a largo reabre o avançado e solta o rodapé', async () => {
      const c = await comLargura(390, () => montarCriando());
      expect(avancado(c.el).open).toBe(false);
      await c.disparar(900);
      expect(avancado(c.el).open).toBe(true);
      expect(acoes(c.el).classList.contains('fixo')).toBe(false);
      unmount(c.comp);
    });

    // As quatro ajudas longas (nome curto, endereço, subagentes, janela) viram um "?" que abre o
    // texto. Nada é removido: o <details> esconde por estilo.
    it('no estreito as quatro ajudas longas viram "?" — e no largo não existem', async () => {
      const l = montarCriando();
      await espera();
      expect(l.el.textContent).toContain(m.config_motores_messages());
      expect(perguntas(l.el).length).toBe(0);
      unmount(l.comp);

      const c = await comLargura(390, () => montarCriando());
      expect(perguntas(c.el).length).toBe(4);
      expect(c.el.textContent).toContain(m.config_motores_messages());
      expect(c.el.textContent).toContain(m.config_motores_subagentes_ajuda());
      for (const s of perguntas(c.el)) expect((s.parentElement as HTMLDetailsElement).open).toBe(false);
      unmount(c.comp);
    });

    // Quatro botões com o mesmo nome são quatro botões idênticos para o leitor de tela: quem
    // navega por Tab não sabe qual "?" abre a ajuda do endereço.
    it('cada "?" tem um nome próprio, com o campo dele', async () => {
      const c = await comLargura(390, () => montarCriando());
      const nomes = perguntas(c.el).map((s) => s.getAttribute('aria-label')!);
      expect(nomes.length).toBe(4);
      expect(new Set(nomes).size).toBe(4);
      expect(nomes[0]).toContain(m.config_motores_nome_curto());
      expect(nomes[1]).toContain(m.config_motores_endereco());
      expect(nomes[2]).toContain(m.config_motores_subagentes());
      expect(nomes[3]).toContain(m.config_motores_janela());
      unmount(c.comp);
    });

    it('o "?" do endereço abre e fecha aquela ajuda', async () => {
      const c = await comLargura(390, () => montarCriando());
      // Ordem no formulário: nome curto, endereço, subagentes, janela.
      const det = perguntas(c.el)[1].parentElement as HTMLDetailsElement;
      expect(det.open).toBe(false);
      perguntas(c.el)[1].click();
      await espera();
      expect(det.open).toBe(true);
      perguntas(c.el)[1].click();
      await espera();
      expect(det.open).toBe(false);
      unmount(c.comp);
    });

    // Ajuda de uma linha e aviso continuam à mostra: esconder um aviso atrás de um clique é
    // esconder o motivo de ele existir.
    it('ajuda curta e aviso não viram "?"', async () => {
      const c = await comLargura(390, () => montarCriando());
      const curta = [...c.el.querySelectorAll('.ajuda')]
        .find((x) => x.textContent?.includes(m.config_motores_sessoes_abertas()))!;
      expect(curta).toBeDefined();
      expect(curta.closest('details')).toBeNull();
      unmount(c.comp);

      // Endereço editado sem chave nova: o aviso aparece solto, sem "?" em volta.
      const t = await comLargura(390, () => montar());
      const end = campo(t.el, 'base_url');
      end.value = 'https://outro.exemplo';
      end.dispatchEvent(new Event('input', { bubbles: true }));
      await espera();
      const aviso = [...t.el.querySelectorAll('.ajuda.erro')]
        .find((x) => x.textContent?.includes(m.config_motores_endereco_mudou()))!;
      expect(aviso).toBeDefined();
      expect(aviso.closest('details')).toBeNull();
      unmount(t.comp);
    });

    it('o resumo do avançado só existe no estreito e traz janela, subagentes e raciocínio', async () => {
      const novo = await comLargura(390, () => montarCriando());
      const resumo = () => novo.el.querySelector('.resumo')!.textContent ?? '';
      expect(resumo()).toContain(m.config_motores_padrao());
      expect(resumo()).toContain(m.config_motores_mesmo_principal());
      expect(resumo()).toContain(m.comum_ligado());
      unmount(novo.comp);

      // Motor com janela, subagente próprio e raciocínio desligado.
      const salvo = await comLargura(390, () => montar({
        ...KIMI, context_window: 256000, subagent_model: 'kimi-k2', adaptive_thinking: false,
      }));
      const texto = salvo.el.querySelector('.resumo')!.textContent ?? '';
      expect(texto).toContain('256k');
      expect(texto).toContain('kimi-k2');
      expect(texto).toContain(m.comum_desligado());
      unmount(salvo.comp);

      const largo = montarCriando();
      await espera();
      expect(largo.el.querySelector('.resumo')).toBeNull();
      unmount(largo.comp);
    });

    // O rodapé grudado cobre o que o navegador encosta na borda de baixo ao dar foco; quem afasta
    // a rolagem da faixa é uma regra presa a `.compacto`. happy-dom não mede rolagem, então o que
    // dá para provar é a classe existir no estreito e não existir no largo.
    it('só no estreito a raiz ganha "compacto", que é onde a regra de rolagem se prende', async () => {
      const c = await comLargura(390, () => montarCriando());
      expect(c.el.querySelector('.mf')!.classList.contains('compacto')).toBe(true);
      unmount(c.comp);

      const largo = montarCriando();
      await espera();
      expect(largo.el.querySelector('.mf')!.classList.contains('compacto')).toBe(false);
      unmount(largo.comp);
    });

    // Bloco ainda não medido tem largura 0: sem esta guarda ele nasceria compacto no desktop.
    it('largura 0 não decide nada — continua largo', async () => {
      const c = await comLargura(0, () => montarCriando());
      expect(avancado(c.el).open).toBe(true);
      expect(acoes(c.el).classList.contains('fixo')).toBe(false);
      unmount(c.comp);
    });
  });

  it('cancelar chama onFechar sem gravar', () => {
    const t = montar();
    botao(t.el, m.comum_cancelar()).click();
    expect(t.onFechar).toHaveBeenCalledTimes(1);
    expect(apiMock.putEngine).not.toHaveBeenCalled();
    unmount(t.comp);
  });
});
