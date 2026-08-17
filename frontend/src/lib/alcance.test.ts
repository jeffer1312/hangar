// @vitest-environment happy-dom
// Frases de estado da aba Acesso: a tela confia neste mapeamento tipo+estado → frase
// (ok varia por tipo; falhou/testando/nao_configurado são fixos; não configurado é
// neutro — não é defeito). Nenhuma rede aqui: só a função pura, e a mensagem por baixo.
import { describe, expect, it } from 'vitest';
import { fraseDeEstado, type EnderecoAlcance } from './alcance';

function linha(parcial: Partial<EnderecoAlcance>): EnderecoAlcance {
  return { tipo: 'rede_local', url: 'http://192.168.0.42:5173', estado: 'ok', tempo_ms: 12, ...parcial };
}

describe('fraseDeEstado', () => {
  it('ok da rede local carrega o tempo medido', () => {
    expect(fraseDeEstado(linha({}))).toContain('12');
  });

  it('ok do tailscale e do público usam a frase de fora de casa, com tempo', () => {
    const ts = fraseDeEstado(linha({ tipo: 'tailscale', url: 'https://hangar.tail.ts.net' }));
    const pub = fraseDeEstado(linha({ tipo: 'publico', url: 'https://hangar.example.com' }));
    expect(ts).toContain('12');
    expect(pub).toContain('12');
    expect(pub).toBe(ts);
  });

  it('ok desta máquina não carrega tempo (só vale aqui dentro)', () => {
    expect(fraseDeEstado(linha({ tipo: 'nesta_maquina', url: 'http://127.0.0.1:5173' }))).not.toContain('12');
  });

  it('falhou é fixo e independente do tipo', () => {
    const a = fraseDeEstado(linha({ estado: 'falhou' }));
    const b = fraseDeEstado(linha({ tipo: 'publico', estado: 'falhou' }));
    expect(a).toBe(b);
    expect(a).not.toContain('12');
  });

  it('testando é fixo e sem tempo', () => {
    const t = fraseDeEstado(linha({ estado: 'testando' }));
    expect(t).not.toBe(fraseDeEstado(linha({ estado: 'falhou' })));
    expect(t).not.toContain('12');
  });

  it('nao_configurado é neutro, nunca a frase de falha', () => {
    const nc = fraseDeEstado(linha({ estado: 'nao_configurado', url: '' }));
    expect(nc).not.toBe(fraseDeEstado(linha({ estado: 'falhou' })));
    expect(nc).not.toContain('12');
  });
});