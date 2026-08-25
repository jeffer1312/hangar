// @vitest-environment happy-dom
// A caixa de Atualizar nos quatro estados, e a regra que dá razão a ela existir: durante o restart
// a conexão CAI por desenho, e a tela tem que continuar dizendo "atualizando" em vez de
// "desconectado" — que é a mesma frase que ela usa quando o servidor caiu de verdade.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import AtualizarSheet from './AtualizarSheet.svelte';
import * as m from '../paraglide/messages';
import * as api from '../lib/api';
import type { Atualizacao } from '../lib/types';

function base(over: Partial<Atualizacao> = {}): Atualizacao {
  return {
    versoes: { repo: 'v1-abc', backend: 'v1-abc' },
    atualizacao_disponivel: false,
    mudancas: [],
    passos: [],
    pre_voo: { pode: true, faltando: [] },
    estado: {},
    ...over,
  };
}

let alvo: HTMLElement;
let comp: Record<string, unknown> | null = null;

function montar(props: Record<string, unknown> = {}) {
  alvo = document.createElement('div');
  document.body.appendChild(alvo);
  comp = mount(AtualizarSheet, { target: alvo, props: { open: true, onClose: () => {}, ...props } });
  return alvo;
}

beforeEach(() => {
  vi.restoreAllMocks();
});

afterEach(() => {
  if (comp) unmount(comp);
  comp = null;
  alvo?.remove();
});

describe('em dia', () => {
  it('mostra uma versão só quando disco e servidor batem', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(base());
    montar();
    await tick();
    await tick();
    const txt = document.body.textContent ?? '';
    expect(txt).toContain(m.atualizar_em_dia_titulo());
    // A segunda versão é diagnóstico: quem só usa não precisa dela quando as duas são iguais.
    expect(txt).not.toContain(m.atualizar_versao_servidor());
  });

  it('sem commit novo, mas com o servidor atrasado, ainda diz o que fazer', async () => {
    // O caso de quem puxou pela linha de comando e não reiniciou: não há nada a baixar, mas a
    // máquina não está rodando o que tem no disco. Sem isto a informação existia e era inalcançável.
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ atualizacao_disponivel: false, versoes: { repo: 'v2-novo', backend: 'v1-velho' } }),
    );
    montar();
    await tick();
    await tick();
    expect(document.body.textContent ?? '').toContain(m.atualizar_precisa_reiniciar());
  });

  it('mostra as duas quando divergem, com o aviso de reiniciar', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ versoes: { repo: 'v2-novo', backend: 'v1-velho' } }),
    );
    montar();
    await tick();
    await tick();
    const txt = document.body.textContent ?? '';
    expect(txt).toContain(m.atualizar_versao_servidor());
    expect(txt).toContain('v1-velho');
    expect(txt).toContain(m.atualizar_precisa_reiniciar());
  });
});

describe('versão nova', () => {
  const dez = Array.from({ length: 10 }, (_, i) => ({ sha: `s${i}`, titulo: `mudança ${i}` }));

  it('lista as novidades e o botão', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ atualizacao_disponivel: true, mudancas: dez.slice(0, 2) }),
    );
    montar();
    await tick();
    await tick();
    const txt = document.body.textContent ?? '';
    expect(txt).toContain(m.atualizar_disponivel_titulo());
    expect(txt).toContain('mudança 0');
    expect(txt).toContain(m.atualizar_botao());
  });

  it('corta em 5 e resume o resto', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ atualizacao_disponivel: true, mudancas: dez }),
    );
    montar();
    await tick();
    await tick();
    const txt = document.body.textContent ?? '';
    expect(txt).toContain('mudança 4');
    expect(txt).not.toContain('mudança 5');
    expect(txt).toContain(m.atualizar_e_mais({ n: 5 }));
  });

  it('não lista os passos como tarefa de quem usa', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({
        atualizacao_disponivel: true,
        mudancas: dez.slice(0, 1),
        passos: [{ id: 'p1', titulo: 'Rodar o instalador', texto: '' }],
      }),
    );
    montar();
    await tick();
    await tick();
    // Passo sem texto próprio não aparece: o que a atualização precisa fazer é problema do app.
    expect(document.body.textContent ?? '').not.toContain('Rodar o instalador');
  });

  it('avisa quantas sessões estão trabalhando', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ atualizacao_disponivel: true, mudancas: dez.slice(0, 1) }),
    );
    montar({ trabalhando: 2 });
    await tick();
    await tick();
    expect(document.body.textContent ?? '').toContain(m.atualizar_sessoes_trabalhando({ n: 2 }));
  });

  it('o botão chama o início', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ atualizacao_disponivel: true, mudancas: dez.slice(0, 1) }),
    );
    const ini = vi.spyOn(api, 'iniciarAtualizacao').mockResolvedValue({ ok: true, pid: 7 });
    montar();
    await tick();
    await tick();
    const botao = [...document.querySelectorAll('button')]
      .find((b) => b.textContent?.trim() === m.atualizar_botao());
    botao?.click();
    await tick();
    expect(ini).toHaveBeenCalled();
  });
});

describe('atualizando', () => {
  it('mostra a etapa e a barra', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ estado: { fase: 'rodando', passo: 3, total: 5, texto: 'Instalando dependências' } }),
    );
    montar();
    await tick();
    await tick();
    const txt = document.body.textContent ?? '';
    expect(txt).toContain(m.atualizar_rodando_titulo());
    expect(txt).toContain('Instalando dependências');
    expect(txt).toContain(m.atualizar_rodando_sub({ passo: 3, total: 5 }));
  });

  it('a conexão caindo NÃO vira "desconectado" enquanto roda', async () => {
    // O restart derruba a conexão por desenho. Se isto virasse mensagem de erro, a tela diria a
    // mesma coisa que diz quando o servidor caiu de verdade — a confusão que este desenho evita.
    const spy = vi.spyOn(api, 'getAtualizacao');
    spy.mockResolvedValueOnce(base({ estado: { fase: 'rodando', passo: 2, total: 5, texto: 'x' } }));
    spy.mockRejectedValue(new Error('Failed to fetch'));
    montar();
    await tick();
    await tick();
    expect(document.body.textContent ?? '').toContain(m.atualizar_rodando_titulo());
    expect(document.body.textContent ?? '').not.toContain('Failed to fetch');
  });
});

describe('ciclo de vida', () => {
  it('servidor demorando pra voltar NÃO para o acompanhamento', async () => {
    // O caso comum, não a borda: restart de systemd passa dos 2s do intervalo com folga. Antes,
    // qualquer tique que não visse `rodando` encerrava o acompanhamento — a barra congelava sem
    // erro nenhum e nada mais reagendava.
    vi.useFakeTimers();
    try {
      const spy = vi.spyOn(api, 'getAtualizacao');
      spy.mockResolvedValueOnce(base({ estado: { fase: 'rodando', passo: 4, total: 5, texto: 'x' } }));
      spy.mockRejectedValue(new Error('Failed to fetch'));   // servidor reiniciando, demorado
      const spyIni = vi.spyOn(api, 'iniciarAtualizacao').mockResolvedValue({ ok: true, pid: 1 });
      montar({ trabalhando: 0 });
      await vi.advanceTimersByTimeAsync(0);

      await vi.advanceTimersByTimeAsync(6000);
      const depoisDeSeisSegundos = spy.mock.calls.length;
      await vi.advanceTimersByTimeAsync(6000);
      expect(spy.mock.calls.length).toBeGreaterThan(depoisDeSeisSegundos);
      expect(document.body.textContent ?? '').not.toContain('Failed to fetch');
      expect(spyIni).toBeDefined();
    } finally {
      vi.useRealTimers();
    }
  });

  it('um tique que estoura NÃO mata o acompanhamento', async () => {
    // A corrente de setTimeout tinha ponto único de falha: um elo que não reagendasse (exceção,
    // await pendurado, retorno cedo) matava tudo calado, e a barra congelava na etapa em que
    // estava. Aconteceu em produção com a tela parada em "Etapa 4 de 5".
    vi.useFakeTimers();
    try {
      const spy = vi.spyOn(api, 'getAtualizacao');
      spy.mockResolvedValueOnce(base({ estado: { fase: 'rodando', passo: 4, total: 5, texto: 'x' } }));
      spy.mockRejectedValueOnce(new Error('caiu'));
      spy.mockImplementationOnce(() => { throw new Error('estourou sincrono'); });
      spy.mockResolvedValue(base({ estado: { fase: 'rodando', passo: 4, total: 5, texto: 'x' } }));
      montar();
      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(8000);
      const ate = spy.mock.calls.length;
      await vi.advanceTimersByTimeAsync(8000);
      expect(spy.mock.calls.length).toBeGreaterThan(ate);
    } finally {
      vi.useRealTimers();
    }
  });

  it('mostra o tempo correndo na etapa', async () => {
    // O `npm ci --silent` não imprime NADA: sem relógio, a tela é idêntica a uma travada durante
    // o minuto inteiro em que ele roda.
    const inicio = new Date(Date.now() - 75_000).toISOString();
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ estado: { fase: 'rodando', passo: 4, total: 5,
                       texto: 'Instalando dependências', etapa_inicio: inicio } }),
    );
    montar();
    await tick();
    await tick();
    expect(document.body.textContent ?? '').toContain('1min 15s');
  });

  it('não fecha enquanto está atualizando', async () => {
    // Fechar no meio não interrompe nada (o motor roda fora do navegador), mas some com a única
    // janela que mostra o que está acontecendo — e reabrir depois ninguém descobre sozinho.
    const fechou = vi.fn();
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ estado: { fase: 'rodando', passo: 3, total: 5, texto: 'x' } }),
    );
    montar({ onClose: fechou });
    await tick();
    await tick();
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await tick();
    expect(fechou).not.toHaveBeenCalled();
  });

  it('fecha normalmente quando NÃO está atualizando', async () => {
    const fechou = vi.fn();
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(base());
    montar({ onClose: fechou });
    await tick();
    await tick();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await tick();
    expect(fechou).toHaveBeenCalled();
  });

  it('mostra o log dos comandos quando existe', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ estado: { fase: 'rodando', passo: 4, total: 5, texto: 'Instalando dependências',
                       log: ['$ bash install.sh --update', 'npm ci: ok'] } }),
    );
    montar();
    await tick();
    await tick();
    // ABERTO por padrão: a caixinha existe pro minuto em que a etapa parece travada, e exigir um
    // clique ali é esconder a resposta bem na hora da pergunta.
    expect(document.body.textContent ?? '').toContain('$ bash install.sh --update');
    expect(document.body.textContent ?? '').toContain(m.atualizar_esconder_log());

    const botao = [...document.querySelectorAll('button')]
      .find((b) => b.textContent?.trim() === m.atualizar_esconder_log());
    botao?.click();
    await tick();
    expect(document.body.textContent ?? '').not.toContain('$ bash install.sh --update');
  });

  it('desmontar o componente para o polling', async () => {
    // Sem isto o timer sobrevive à destruição do DesktopShell (navegar pra Custos/Arquivo) e, ao
    // terminar a atualização, chama location.reload() na cara de quem já estava noutra tela.
    vi.useFakeTimers();
    try {
      const spy = vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
        base({ estado: { fase: 'rodando', passo: 1, total: 5, texto: 'x' } }),
      );
      montar();
      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(2100);
      const antes = spy.mock.calls.length;
      expect(antes).toBeGreaterThan(1);   // estava mesmo pollando

      unmount(comp!);
      comp = null;
      await vi.advanceTimersByTimeAsync(10_000);
      expect(spy.mock.calls.length).toBe(antes);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('deu erro', () => {
  it('mostra o erro, o estado da máquina e a branch de resgate', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({
        estado: {
          fase: 'pronto', ok: false, erro: 'Rollup failed to resolve import',
          voltou: true, resgate: 'resgate/2026-08-25-1432',
        },
      }),
    );
    montar();
    await tick();
    await tick();
    const txt = document.body.textContent ?? '';
    expect(txt).toContain(m.atualizar_falhou_titulo());
    expect(txt).toContain('Rollup failed to resolve import');
    expect(txt).toContain(m.atualizar_falhou_voltou());
    expect(txt).toContain(m.atualizar_resgate({ branch: 'resgate/2026-08-25-1432' }));
  });

  it('sem rollback, diz que parou no meio', async () => {
    vi.spyOn(api, 'getAtualizacao').mockResolvedValue(
      base({ estado: { fase: 'pronto', ok: false, erro: 'x', voltou: false } }),
    );
    montar();
    await tick();
    await tick();
    expect(document.body.textContent ?? '').toContain(m.atualizar_falhou_parou());
  });
});
