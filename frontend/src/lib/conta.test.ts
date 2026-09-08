import { describe, it, expect } from 'vitest';
import * as m from '../paraglide/messages';
import { chipDaConta } from './conta';

describe('chipDaConta', () => {
  it('conta nomeada vira o sufixo da pasta, com cor estável', () => {
    const a = chipDaConta('claude:/home/x/.claude-claude-200-3');
    expect(a?.nome).toBe('claude-200-3');
    expect(a?.label).toBe('200-3');   // o `claude-` repetido sai do chip, não do nome
    expect(chipDaConta('claude:/home/x/.claude-claude-200-3/')?.cor).toBe(a?.cor);
    expect(chipDaConta('claude:/home/x/.claude-jefferson')?.label).toBe('jefferson');
    expect(chipDaConta('claude:/home/x/.claude-200-01')?.label).toBe('200-01');
  });

  it('a conta padrão (~/.claude) ganha o rótulo traduzido', () => {
    // O vitest roda na locale do sistema: compara com a mensagem, não com o literal.
    expect(chipDaConta('claude:/home/x/.claude')?.label).toBe(m.conta_padrao());
    expect(chipDaConta('claude:/home/x/.claude')?.label).not.toBe('.claude');
  });

  it('motor, codex e ausência não viram chip (já têm o seu)', () => {
    expect(chipDaConta('chave:deepseek')).toBeNull();
    expect(chipDaConta('codex:/home/x/.codex')).toBeNull();
    expect(chipDaConta(null)).toBeNull();
  });
});
