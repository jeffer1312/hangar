// Faixa de cota do rodapé do desktop (Task 9): módulo puro que decide se a faixa aparece e o
// que cada conta desenha. Nenhuma rede aqui — recebe ContaEstado[] e devolve o quê mostrar.
import { describe, it, expect } from 'vitest';
import { faixaDeCota, nivelDePct, VELHA_APOS_S } from './cota';
import type { ContaEstado } from './contaEstado';

// Uma conta 'lida' com a linha da statusline crua (o mock da faixa usa ⚡5h/📅7d).
function lida(label: string, linha: string, ts: number | null = 1_000, idade_s: number | null = 5): ContaEstado {
  return {
    path: `/home/u/${label}`, label, active: false,
    login: { estado: 'ok', loggedIn: true },
    limite: { estado: 'lido', linha, ts, idade_s },
  };
}
// Conta legível (tem leitura de limite) com as duas janelas — o caso típico do mock.
const COM_JANELAS = (label: string, cinco: string, sete: string) =>
  lida(label, `🤖 Opus (high✦) │ ⚡5h:${cinco}% 📅7d:${sete}% │ 💵 $1.20`);

function semLeitura(label: string): ContaEstado {
  return {
    path: `/home/u/${label}`, label, active: false,
    login: { estado: 'ok', loggedIn: false },
    limite: { estado: 'sem_leitura' },
  };
}

describe('faixaDeCota — a regra de aparecer', () => {
  it('zero contas legíveis -> não aparece (null)', () => {
    expect(faixaDeCota([])).toBeNull();
  });
  it('uma conta legível -> não aparece (o número dela já está na statusline da sessão)', () => {
    expect(faixaDeCota([COM_JANELAS('jefferson', '64', '83')])).toBeNull();
  });
  it('duas contas legíveis -> aparece com as duas', () => {
    const r = faixaDeCota([
      COM_JANELAS('jefferson', '64', '83'), COM_JANELAS('claude-200-1', '5', '58'),
    ])!;
    expect(r).toHaveLength(2);
    expect(r.map((c) => c.label)).toEqual(['jefferson', 'claude-200-1']);
  });
  it('conta sem leitura não conta pro total e não vira zero', () => {
    // Uma lida + uma sem leitura = só uma legível -> não aparece.
    expect(faixaDeCota([COM_JANELAS('jefferson', '64', '83'), semLeitura('nova')])).toBeNull();
    // Duas lidas + uma sem leitura = aparece com as duas lidas, a sem leitura fora.
    const r = faixaDeCota([
      COM_JANELAS('jefferson', '64', '83'), semLeitura('nova'), COM_JANELAS('claude-200-1', '5', '58'),
    ])!;
    expect(r).toHaveLength(2);
    expect(r.map((c) => c.label)).toEqual(['jefferson', 'claude-200-1']);
  });
  it('conta lida com linha sem janela parseável também não conta como legível', () => {
    const linhaSemJanela = lida('tema-custom', '🤖 Opus (high✦) │ 💵 $1.20');
    expect(faixaDeCota([COM_JANELAS('jefferson', '64', '83'), linhaSemJanela])).toBeNull();
    const r = faixaDeCota([
      COM_JANELAS('jefferson', '64', '83'), linhaSemJanela, COM_JANELAS('claude-200-1', '5', '58'),
    ])!;
    expect(r).toHaveLength(2);
  });
});

describe('faixaDeCota — o que cada conta desenha', () => {
  it('janelas 5h e 7d com porcentagem e nível de cor', () => {
    const r = faixaDeCota([
      COM_JANELAS('jefferson', '64', '83'), COM_JANELAS('outra', '96', '5'),
    ])!;
    const jeff = r.find((c) => c.label === 'jefferson')!;
    expect(jeff.cincoH).toEqual({ rotulo: '5h', pct: 64, nivel: 'normal' });
    expect(jeff.seteD).toEqual({ rotulo: '7d', pct: 83, nivel: 'alerta' });
    const outra = r.find((c) => c.label === 'outra')!;
    expect(outra.cincoH!.nivel).toBe('cheio');
    expect(outra.seteD!.pct).toBe(5);
  });
  it('cor só acima de 80: 80 é normal, 81 alerta, 90 alerta, 91 cheio', () => {
    expect(nivelDePct(80)).toBe('normal');
    expect(nivelDePct(81)).toBe('alerta');
    expect(nivelDePct(90)).toBe('alerta');
    expect(nivelDePct(91)).toBe('cheio');
  });
  it('janela faltando na linha -> o par não é desenhado (null), não vira 0%', () => {
    const so5h = lida('so5h', '🤖 Opus (high✦) │ ⚡5h:64% │ 💵 $1.20');
    const r = faixaDeCota([COM_JANELAS('jefferson', '64', '83'), so5h])!;
    const c = r.find((x) => x.label === 'so5h')!;
    expect(c.cincoH).not.toBeNull();
    expect(c.seteD).toBeNull();
  });
});

describe('faixaDeCota — leitura velha parece velha', () => {
  it('leitura mais velha que VELHA_APOS_S é marcada velha, com a idade em segundos', () => {
    const agora = 10_000;
    const velha = COM_JANELAS('velha', '5', '58');
    velha.limite = { estado: 'lido', linha: velha.limite.linha!, ts: agora - VELHA_APOS_S - 1, idade_s: VELHA_APOS_S + 1 };
    const fresca = COM_JANELAS('fresca', '64', '83');
    fresca.limite = { estado: 'lido', linha: fresca.limite.linha!, ts: agora - 5, idade_s: 5 };
    const r = faixaDeCota([velha, fresca])!;
    expect(r.find((c) => c.label === 'velha')!.velha).toBe(true);
    expect(r.find((c) => c.label === 'fresca')!.velha).toBe(false);
    expect(r.find((c) => c.label === 'velha')!.idade_s).toBe(VELHA_APOS_S + 1);
  });
  // O VELHA_APOS_S vive no módulo: se ele mudar, o teste acima continua valendo, mas a constante
  // é o lugar onde o limiar é decidido. Esse teste congela o valor para ninguém mudar por engano.
  it('VELHA_APOS_S é 10 minutos (600 s)', () => {
    expect(VELHA_APOS_S).toBe(600);
  });
});