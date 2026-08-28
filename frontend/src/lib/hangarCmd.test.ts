import { describe, it, expect } from 'vitest';
import { lerComandoHangar, lerFerramentaClaude } from './hangarCmd';

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

  it('flag ANTES do nome não vira alvo: o alvo é a sessão, nunca a mensagem', () => {
    // Relatado com print: `hangar-send --tmux <sessao> "<msg longa>"` desenhava o botão
    // "Abrir Chame AskUserQuestion uma vez com multiSelect true, 1 pergunta e 4 opcoes…" — a flag
    // engolia o nome como se fosse valor dela, e a mensagem inteira virava o alvo.
    const a = lerComandoHangar(
      'hangar-send --tmux teste-picker "Chame AskUserQuestion uma vez com multiSelect true"',
      'entregue -> teste-picker', false,
    );
    expect(a?.alvo).toBe('teste-picker');
    expect(a?.texto).toBe('Chame AskUserQuestion uma vez com multiSelect true');
  });

  it('nome com aspas e flag junto continua sendo o nome', () => {
    const a = lerComandoHangar('hangar-send --tmux "sessao com espaco" "oi"', 'entregue -> x', false);
    expect(a?.alvo).toBe('sessao com espaco');
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

// Saídas COPIADAS de uma chamada real (28/08/2026, claude 2.1.224) — o shape do `ListAgents` tem
// separador `·` e sufixo `started Xh ago`, que uma amostra inventada erraria.
const LISTA = [
  'This session is hangar-78 [543645] — the name other sessions use to message it (it is not listed below).',
  '',
  'Peer sessions (2):',
  '  hangar-b2 [5f1ae0]  ·  interactive  ·  idle  ·  tmux hangar-2:@1921.%2050  ·  started 19h ago',
  '  app-web-0f [a47c75]  ·  interactive  ·  busy  ·  tmux tarefa-28:@2807.%2996  ·  started 7m ago',
].join('\n');

describe('lerFerramentaClaude', () => {
  it('SendMessage entregue: alvo, texto e a marca da via', () => {
    const a = lerFerramentaClaude(
      'SendMessage',
      { to: 'hangar-b2', message: 'não era orquestração' },
      '{"success":true,"message":"\\"não era orquestração\\" → hangar-b2 (another Claude session on this machine)","msg_id":"535b"}',
      false,
    );
    expect(a).toMatchObject({ verbo: 'recado', via: 'claude', alvo: 'hangar-b2', entregue: true });
    expect(a?.erro).toBeUndefined();
  });

  it('resultado sem `success` não vira "entregue" por otimismo', () => {
    const a = lerFerramentaClaude('SendMessage', { to: 'x', message: 'oi' }, 'ok', false);
    expect(a?.entregue).toBe(false);
    expect(a?.erro).toBeUndefined();
  });

  it('`success: false` é erro mesmo sem o tool_result vir marcado', () => {
    const a = lerFerramentaClaude('SendMessage', { to: 'x' }, '{"success":false,"error":"no such agent"}', false);
    expect(a?.erro).toContain('no such agent');
  });

  it('`success` fora do booleano não vira erro — cartão vermelho sobre recado entregue é pior', () => {
    for (const bruto of ['{"success":"true","message":"ok"}', '{"success":1}']) {
      const a = lerFerramentaClaude('SendMessage', { to: 'x' }, bruto, false);
      expect(a?.erro).toBeUndefined();
      expect(a?.entregue).toBe(false);
    }
  });

  it('ListAgents: uma linha por sessão, busy vira working, e o nome desta sessão sai à parte', () => {
    const a = lerFerramentaClaude('ListAgents', {}, LISTA, false);
    expect(a?.eu).toBe('hangar-78');
    expect(a?.sessoes).toEqual([
      { nome: 'hangar-b2', estado: 'idle', cwd: '', extra: '19h ago' },
      { nome: 'app-web-0f', estado: 'working', cwd: '', extra: '7m ago' },
    ]);
  });

  it('outra ferramenta qualquer não vira cartão', () => {
    expect(lerFerramentaClaude('Bash', { command: 'ls' }, 'a  b', false)).toBeNull();
    expect(lerFerramentaClaude(null, null, '', false)).toBeNull();
  });
});
