// @vitest-environment happy-dom
// Faixa de cota do rodapé do desktop: uma caixa por CONTA (janelas do provedor, cor acima de
// 80%, idade na leitura velha), link leva à aba Contas por callback — o DesktopShell desvia para
// a rota (?config=contas), nunca importando o componente ContasSettings.
//
// A faixa é móvel do app: ela NÃO some porque uma conta perdeu a leitura, nem porque o refetch
// falhou. Sumir do rodapé faz o usuário procurar por ela.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import QuotaStrip from './QuotaStrip.svelte';
import * as m from '../paraglide/messages';
import * as contaEstadoLib from '../lib/contaEstado';
import type { CotaConta } from '../lib/contaEstado';

vi.mock('../lib/contaEstado', async (importOriginal) => {
  const real = await importOriginal<typeof import('../lib/contaEstado')>();
  return { ...real, listarCotas: vi.fn() };
});

const cotaMock = vi.mocked(contaEstadoLib);

const agoraS = () => Date.now() / 1000;

function lida(
  label: string, cinco: number, sete: number,
  extra: Partial<CotaConta> = {},
): CotaConta {
  return {
    id: `claude:/home/u/${label}`, label, provedor: 'claude', ativa: false, estado: 'lida',
    janelas: [
      // +30s de folga: com 80 min EXATOS, o tempo que o teste leva pra montar já derrubava
      // o piso pra 79 e a asserção virava '↺1h19' — teste instável, não bug do código.
      { rotulo: '5h', pct: cinco, reset_ts: agoraS() + 80 * 60 + 30 },
      { rotulo: '7d', pct: sete, reset_ts: agoraS() + 3 * 86400 },
    ],
    ts: agoraS(), idade_s: 5, ...extra,
  };
}

const EXPIRADA: CotaConta = {
  id: 'claude:/home/u/jefferson', label: 'jefferson', provedor: 'claude', ativa: false,
  estado: 'expirada', janelas: [], ts: null, idade_s: null, motivo: 'token-expirado',
};

function montar(contas: CotaConta[], onIrParaContas = vi.fn()) {
  cotaMock.listarCotas.mockResolvedValue(contas);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(QuotaStrip, { target: el, props: { serverKey: 'srv-1', onIrParaContas } });
  return { el, comp: comp as never, onIrParaContas };
}

beforeEach(() => { vi.clearAllMocks(); });
afterEach(() => { document.body.innerHTML = ''; });

describe('QuotaStrip — o que aparece', () => {
  it('fala com o servidor ATIVO de propósito — alvo null explícito', async () => {
    const t = montar([lida('default', 13, 22), lida('200-01', 26, 87)]);
    await tick(); await tick();
    expect(cotaMock.listarCotas).toHaveBeenCalledWith(null);
    unmount(t.comp);
  });

  it('some só quando NÃO há conta nenhuma', async () => {
    const t = montar([]);
    await tick(); await tick();
    expect(t.el.querySelector('.quota-faixa')).toBeNull();
    unmount(t.comp);
  });

  it('com uma conta só ela continua no rodapé (não pisca)', async () => {
    const t = montar([lida('default', 13, 22)]);
    await tick(); await tick();
    expect(t.el.querySelectorAll('.quota-conta')).toHaveLength(1);
    unmount(t.comp);
  });

  it('cada conta desenha o SEU número — o bug que a rota nova fecha', async () => {
    const t = montar([lida('default', 13, 22), lida('200-01', 26, 87)]);
    await tick(); await tick();
    const contas = [...t.el.querySelectorAll('.quota-conta')];
    expect(contas).toHaveLength(2);
    const nums = contas.map((c) => [...c.querySelectorAll('.quota-v b')].map((n) => n.textContent));
    expect(nums).toEqual([['13%', '22%'], ['26%', '87%']]);
    expect(contas[0].querySelectorAll('.quota-rot')[0].textContent).toBe('5h');
    // A barrinha de progresso saiu de vez: em 2% ela nao desenhava nada e virava ruido.
    expect(contas[0].querySelector('.quota-barra')).toBeNull();
    unmount(t.comp);
  });

  it('conta com credencial vencida aparece nomeada e vazia, nunca como 0%', async () => {
    const t = montar([lida('default', 13, 22), EXPIRADA]);
    await tick(); await tick();
    const contas = [...t.el.querySelectorAll('.quota-conta')];
    expect(contas).toHaveLength(2);
    const vencida = contas[1];
    expect(vencida.querySelector('.quota-nome')!.textContent).toBe('jefferson');
    expect(vencida.querySelector('.quota-v')).toBeNull();
    expect(vencida.querySelector('.quota-vazio')!.textContent).toBe(m.cota_precisa_entrar());
    unmount(t.comp);
  });

  it('sessao-viva NÃO manda abrir sessão — a sessão já está aberta (queixa do usuário, 19/08)', async () => {
    // Conta com sessão rodando e access token vencido: o CLI dela renova sozinho no próximo
    // turno. A frase é "renova sozinha", nunca "abra uma sessão nela" — o usuário leu isso
    // estando DENTRO da sessão.
    const VIVA: CotaConta = { ...EXPIRADA, motivo: 'sessao-viva' };
    const t = montar([VIVA]);
    await tick(); await tick();
    expect(t.el.querySelector('.quota-vazio')!.textContent).toBe(m.cota_sessao_viva());
    unmount(t.comp);
  });

  it('renovacao-falhou continua mandando abrir uma sessão (é o gesto que renova)', async () => {
    const FALHOU: CotaConta = { ...EXPIRADA, motivo: 'renovacao-falhou' };
    const t = montar([FALHOU]);
    await tick(); await tick();
    expect(t.el.querySelector('.quota-vazio')!.textContent).toBe(m.cota_conta_parada());
    unmount(t.comp);
  });

  it('a conta-base do app vem marcada (é a que uma sessão nova vai gastar)', async () => {
    const t = montar([lida('default', 13, 22, { ativa: true }), lida('200-01', 26, 87)]);
    await tick(); await tick();
    const contas = [...t.el.querySelectorAll('.quota-conta')];
    expect(contas[0].getAttribute('class')).toContain('base');
    expect(contas[1].getAttribute('class')).not.toContain('base');
    unmount(t.comp);
  });
});

describe('QuotaStrip — cor, reset e idade', () => {
  it('cor acima de 80% na barra e no número', async () => {
    const t = montar([lida('jefferson', 64, 83), lida('outra', 96, 5)]);
    await tick(); await tick();
    const contas = [...t.el.querySelectorAll('.quota-conta')];
    const v7dJeff = contas[0].querySelectorAll('.quota-v')[1];
    expect(v7dJeff.getAttribute('class')).toContain('alerta');
    const v5hOutra = contas[1].querySelectorAll('.quota-v')[0];
    expect(v5hOutra.getAttribute('class')).toContain('cheio');
    unmount(t.comp);
  });

  it('TODA janela mostra o reset: a curta conta o tempo, a longa mostra o dia', async () => {
    // Nem 96% nem 22%: o reset não depende de estar apertado — "quando volta" é metade da
    // informação de uma cota.
    const t = montar([lida('jefferson', 4, 22)]);
    await tick(); await tick();
    const conta = t.el.querySelector('.quota-conta')!;
    const resets = [...conta.querySelectorAll('.quota-reset')].map((r) => r.textContent);
    expect(resets).toHaveLength(2);
    expect(resets[0]).toBe('↺1h20');                  // 5h -> quanto falta
    expect(resets[1]).toMatch(/^↺\p{L}+ \d+h$/u);     // 7d -> dia da semana + hora
    unmount(t.comp);
  });

  it('a conta de leitura velha esmaece e carrega a idade (dado velho parece velho)', async () => {
    // ts de 2 h atrás (bem acima de VELHA_APOS_S = 600 s). O componente re-deriva a idade do ts.
    const t = montar([
      lida('jefferson', 64, 83),
      lida('velha', 5, 58, { ts: agoraS() - 2 * 3600 }),
    ]);
    await tick(); await tick();
    const contas = [...t.el.querySelectorAll('.quota-conta')];
    const velha = contas.find((c) => c.querySelector('.quota-nome')!.textContent === 'velha')!;
    expect(velha.getAttribute('class')).toContain('velha');
    expect(velha.querySelector('.quota-idade')!.textContent).toContain(m.cota_idade({ n: '2 h' }));
    const fresca = contas.find((c) => c.querySelector('.quota-nome')!.textContent === 'jefferson')!;
    expect(fresca.getAttribute('class')).not.toContain('velha');
    expect(fresca.querySelector('.quota-idade')).toBeNull();
    unmount(t.comp);
  });
});

describe('QuotaStrip — o link para a aba Contas', () => {
  it('clique no link chama o callback (o DesktopShell desvia para a rota ?config=contas)', async () => {
    const onIrParaContas = vi.fn();
    const t = montar([lida('default', 13, 22), lida('200-01', 26, 87)], onIrParaContas);
    await tick(); await tick();
    const link = t.el.querySelector<HTMLButtonElement>('.quota-link')!;
    expect(link.textContent).toBe(m.contas_titulo());
    link.click();
    expect(onIrParaContas).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });
});

describe('QuotaStrip — trilho rolável e falha de rede', () => {
  it('as contas vivem num trilho rolável (o nome não encolhe até sumir em janela estreita)', async () => {
    const t = montar([lida('default', 13, 22), lida('200-01', 26, 87)]);
    await tick(); await tick();
    const faixa = t.el.querySelector('.quota-faixa')!;
    const trilho = faixa.querySelector('.quota-trilho')!;
    expect(trilho.querySelectorAll('.quota-conta')).toHaveLength(2);
    expect(trilho.querySelector('.quota-link')).toBeNull();
    expect(faixa.querySelector('.quota-fim')!.querySelector('.quota-link')).not.toBeNull();
    unmount(t.comp);
  });

  it('falha de rede no refetch não apaga a faixa — o dado bom continua', async () => {
    vi.useFakeTimers();
    try {
      const t = montar([lida('default', 13, 22), lida('200-01', 26, 87)]);
      await tick(); await tick();
      expect(t.el.querySelector('.quota-faixa')).not.toBeNull();
      cotaMock.listarCotas.mockRejectedValueOnce(new Error('rede caiu'));
      vi.advanceTimersByTime(60_000);
      await tick(); await tick();
      const faixa = t.el.querySelector('.quota-faixa')!;
      expect(faixa).not.toBeNull();
      expect(faixa.querySelectorAll('.quota-conta')).toHaveLength(2);
      unmount(t.comp);
    } finally {
      vi.useRealTimers();
    }
  });
});
