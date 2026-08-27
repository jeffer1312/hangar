import { describe, it, expect } from 'vitest';
import { lerRecadoOrq } from './orqRecado';

// Texto REAL do backend (`api._recado_arbitro`), sem o prefixo `[painel: orquestração]` — é o que o
// `parsePeerMessage` entrega ao componente.
const CAUDA =
  ' Aplicação, papel a papel: sessão desse papel PARADA (idle) → feche-a e abra outra já na ' +
  'configuração nova (o Claude não troca conta/modelo com a sessão aberta); sessão TRABALHANDO → ' +
  'deixe terminar a tarefa atual e a próxima sessão desse papel nasce na nova. A linha já está ' +
  'gravada: não reescreva a tabela. Se o papel for o seu (árbitro): termine a tarefa em curso, ' +
  "escreva no seu registro (o diário do grupo) a seção 'Passagem para o árbitro seguinte' — rito " +
  "'Sucessão do árbitro' da skill.";

const recado = (linhas: string) =>
  'A configuração de modelos do grupo mudou no painel: ' + linhas +
  '. Releia `/home/jefferson/.claude/.hangar-pair/regras-3c9eb0ba.md`.' + CAUDA;

describe('lerRecadoOrq', () => {
  it('um papel: tira provider, conta, modelo, esforço e o caminho das regras', () => {
    const r = lerRecadoOrq(recado(
      '`executor` agora é provider `claude`, conta `claude-200-3`, modelo `opus[1m]`, esforço `medium`',
    ));
    expect(r?.papeis).toEqual([
      { papel: 'executor', provider: 'claude', conta: 'claude-200-3', modelo: 'opus[1m]', esforco: 'medium' },
    ]);
    expect(r?.regras).toBe('/home/jefferson/.claude/.hangar-pair/regras-3c9eb0ba.md');
    expect(r?.sucessao).toBe(false);
  });

  it('vários papéis numa escrita só, e `-` vira campo vazio', () => {
    const r = lerRecadoOrq(recado(
      '`executor` agora é provider `claude`, conta `claude-200-3`, modelo `opus[1m]`, esforço `medium`; ' +
      '`revisor` agora é provider `kimi`, conta `kimi-jefferson`, modelo `-`, esforço `high`',
    ));
    expect(r?.papeis.map((p) => p.papel)).toEqual(['executor', 'revisor']);
    expect(r?.papeis[1].modelo).toBe('');
  });

  it('o papel mudado ser o árbitro liga o rito de sucessão', () => {
    const r = lerRecadoOrq(recado(
      '`árbitro` agora é provider `claude`, conta `claude-200-1`, modelo `opus`, esforço `high`',
    ));
    expect(r?.sucessao).toBe(true);
  });

  it('formato desconhecido volta null — a bolha de texto continua valendo', () => {
    expect(lerRecadoOrq('A configuração de modelos do grupo mudou no painel: sei lá. Releia `/x.md`.')).toBeNull();
    expect(lerRecadoOrq('recado qualquer de outra sessão')).toBeNull();
  });

  it('meia lista não vira cartão: uma linha ilegível derruba o cartão inteiro', () => {
    const r = lerRecadoOrq(recado(
      '`executor` agora é provider `claude`, conta `c`, modelo `opus`, esforço `medium`; `revisor` mudou',
    ));
    expect(r).toBeNull();
  });
});
