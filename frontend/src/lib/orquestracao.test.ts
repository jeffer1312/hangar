import { describe, it, expect } from 'vitest';
import { compararModelo, compararEsforco, compararConta, estadoDoPapel, casarViva, familiaDe, type Papel } from './orquestracao';
import type { SessionInfo } from './types';

const papel = (p: Partial<Papel> = {}): Papel => ({
  papel: 'executor', sessao: 'pm-t*', provider: 'claude', conta: '200-01',
  modelo: 'opus[1m]', esforco: 'medium', viva: null, ...p,
});
const sessao = (s: Partial<SessionInfo> = {}): SessionInfo =>
  ({ name: 'pm-t9', state: 'idle', provider: 'claude', ...s } as SessionInfo);

describe('familiaDe', () => {
  it('lê família e marcador 1M de id e de rótulo', () => {
    expect(familiaDe('opus[1m]')).toEqual({ familia: 'opus', um: true });
    expect(familiaDe('Opus4.8·1M')).toEqual({ familia: 'opus', um: true });
    expect(familiaDe('Fable 5')).toEqual({ familia: 'fable', um: false });
    expect(familiaDe('apikey/k3')).toEqual({ familia: 'k', um: false });
    expect(familiaDe('')).toBeNull();
  });
});

describe('compararModelo', () => {
  it('Opus4.8·1M vs opus[1m] é igual', () => {
    expect(compararModelo('opus[1m]', 'Opus4.8·1M')).toBe('igual');
  });
  it('Fable 5 vs opus é divergente', () => {
    expect(compararModelo('opus', 'Fable 5')).toBe('divergente');
  });
  it('opus sem 1M vs Opus·1M é divergente', () => {
    expect(compararModelo('opus', 'Opus4.8·1M')).toBe('divergente');
  });
  it('sem medição é nao_medido, nunca divergente', () => {
    expect(compararModelo('opus[1m]', null)).toBe('nao_medido');
    expect(compararModelo('opus[1m]', '')).toBe('nao_medido');
  });
});

describe('compararEsforco', () => {
  it('high vs medium diverge; med ≈ medium', () => {
    expect(compararEsforco('medium', 'high')).toBe('divergente');
    expect(compararEsforco('medium', 'med')).toBe('igual');
    expect(compararEsforco('medium', undefined)).toBe('nao_medido');
  });
});

describe('compararConta', () => {
  it('só o Claude é medido; Pi/Kimi ficam nao_medido', () => {
    expect(compararConta(papel(), sessao({ conta: 'claude:/home/x/.claude-200-01' }))).toBe('igual');
    expect(compararConta(papel(), sessao({ conta: 'claude:/home/x/.claude-claude-200-3' }))).toBe('divergente');
    expect(compararConta(papel({ conta: 'padrao' }), sessao({ conta: 'claude:/home/x/.claude' }))).toBe('igual');
    expect(compararConta(papel({ provider: 'kimi', conta: 'apikey' }), sessao({ provider: 'kimi', conta: 'kimi:apikey' }))).toBe('nao_medido');
    expect(compararConta(papel({ provider: 'pi' }), sessao({ provider: 'pi', conta: null }))).toBe('nao_medido');
  });
});

describe('estadoDoPapel', () => {
  it('statusline ausente → tudo nao_medido, sem vermelho', () => {
    const e = estadoDoPapel(papel(), sessao({ status_line: null }));
    expect(e.viva).toBe(true);
    expect(e.modelo).toBe('nao_medido');
    expect(e.divergente).toBe(false);
  });
  it('sessão morta → viva false', () => {
    expect(estadoDoPapel(papel(), null).viva).toBe(false);
  });
  it('rodando em high com contrato medium → divergente', () => {
    const e = estadoDoPapel(papel(), sessao({ status_line: '🤖 Opus4.8·1M (high✦) │ 📁 x [main]' }));
    expect(e.modelo).toBe('igual');
    expect(e.esforco).toBe('divergente');
    expect(e.esforcoMedido).toBe('high');
    expect(e.divergente).toBe(true);
  });
});

describe('casarViva', () => {
  const vivas = [sessao({ name: 'pm-t8', last_activity: 10 }), sessao({ name: 'pm-t9', last_activity: 20 }), sessao({ name: 'outra' })];
  it('exato e prefixo com * (mais recente)', () => {
    expect(casarViva('outra', vivas)?.name).toBe('outra');
    expect(casarViva('pm-t*', vivas)?.name).toBe('pm-t9');
    expect(casarViva('nada*', vivas)).toBeNull();
    expect(casarViva('', vivas)).toBeNull();
  });
});
