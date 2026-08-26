import { describe, it, expect } from 'vitest';
import { lerComandoHangar } from './hangarCmd';

describe('lerComandoHangar', () => {
  it('sessão criada: tira nome, cwd, provider e marca worktree', () => {
    const a = lerComandoHangar(
      'hangar-send --new sessao-a "/home/u/app-web/.claude/worktrees/sessao-a" --provider kimi',
      'sessão criada: sessao-a (/home/u/app-web/.claude/worktrees/sessao-a) [kimi] — fala com ela via: hangar-send sessao-a "msg"',
      false,
    );
    expect(a).toMatchObject({ verbo: 'criar', alvo: 'sessao-a', provider: 'kimi', worktree: true });
    expect(a?.erro).toBeUndefined();
  });

  it('checkout normal não vira worktree', () => {
    const a = lerComandoHangar(
      'hangar-send --new x /home/u/app-web',
      'sessão criada: x (/home/u/app-web) — fala com ela via: hangar-send x "msg"',
      false,
    );
    expect(a?.worktree).toBe(false);
  });

  it('motor inexistente vira erro, com a saída crua junto', () => {
    const a = lerComandoHangar('hangar-send --new x /tmp --engine glm-antigo', 'erro HTTP 400: motor invalido', true);
    expect(a?.verbo).toBe('criar');
    expect(a?.erro).toContain('motor invalido');
  });

  it('recado: alvo e texto, e sabe quando ficou na fila', () => {
    const a = lerComandoHangar('hangar-send sessao-b "confere o contrato"', 'na fila -> sessao-b (sessão ocupada; entrega no próximo idle)', false);
    expect(a).toMatchObject({ verbo: 'recado', alvo: 'sessao-b', texto: 'confere o contrato', enfileirado: true });
  });

  it('recado entregue não é fila', () => {
    const a = lerComandoHangar('hangar-send hangar-2 "oi"', 'entregue -> hangar-2', false);
    expect(a?.enfileirado).toBe(false);
  });

  it('--list vira lista de sessões', () => {
    const saida = [
      'sessao-b             working         /home/u/app-web',
      'sessao-a                 idle            /home/j/worktrees/sessao-a',
    ].join('\n');
    const a = lerComandoHangar('hangar-send --list', saida, false);
    expect(a?.verbo).toBe('listar');
    expect(a?.sessoes).toHaveLength(2);
    expect(a?.sessoes?.[0]).toEqual({ nome: 'sessao-b', estado: 'working', cwd: '/home/u/app-web' });
  });

  it('--pair traz par e tarefa', () => {
    const a = lerComandoHangar('hangar-send --pair sessao-b "unificar os logs"', 'pareado: sessao-a <-> sessao-b (registrado no app)', false);
    expect(a).toMatchObject({ verbo: 'parear', alvo: 'sessao-b', texto: 'unificar os logs' });
  });

  it('alvo entre aspas não vira o texto do recado', () => {
    // O nome com espaço é o único trecho entre aspas — pegá-lo como mensagem faria o cartão dizer
    // que o recado era o próprio nome da sessão.
    const a = lerComandoHangar('hangar-send --pair "sessao com espaco"', 'pareado: x <-> sessao com espaco', false);
    expect(a).toMatchObject({ verbo: 'parear', alvo: 'sessao com espaco' });
    expect(a?.texto).toBeUndefined();
  });

  it('--group registra quem recebeu', () => {
    const a = lerComandoHangar('hangar-send --group "task 3 fechada"', 'aviso enviado ao grupo: a, b, c', false);
    expect(a?.peers).toEqual(['a', 'b', 'c']);
  });

  it('prefixo comum (timeout, env) não esconde o comando', () => {
    expect(lerComandoHangar('timeout 25 hangar-send --list 2>&1 | head -2', 'a  idle  /tmp', false)?.verbo).toBe('listar');
    expect(lerComandoHangar('CP_X=1 hangar-send --list', '', false)?.verbo).toBe('listar');
  });

  it('comando que não é do hangar não vira cartão', () => {
    expect(lerComandoHangar('git status', '', false)).toBeNull();
    expect(lerComandoHangar('echo hangar-send', '', false)).toBeNull();
  });

  it('o nome antigo cp-send continua sendo reconhecido', () => {
    expect(lerComandoHangar('cp-send --list', '', false)?.verbo).toBe('listar');
  });
});
