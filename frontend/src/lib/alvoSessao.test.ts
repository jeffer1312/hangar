import { describe, it, expect } from 'vitest';
import { rotaDoAlvo } from './alvoSessao';
import type { ServerBucket } from '@hangar/core';

function bucket(id: string, label: string, baseUrl: string, nomes: string[]): ServerBucket {
  return {
    server: { id, label, baseUrl, token: 't' },
    sessions: nomes.map((name) => ({ name })),
    error: null,
    loaded: true,
  } as unknown as ServerBucket;
}

const LOCAL = bucket('local', 'notebook', 'http://127.0.0.1:8765', ['Contas', 'jefferson']);
const VPS = bucket('srv-abc', 'servidor-b', 'https://maquina-b.exemplo/', ['thread-admin', 'manager']);
const B = [LOCAL, VPS];

describe('rotaDoAlvo', () => {
  it('nome de sessão do servidor atual abre nele', () => {
    expect(rotaDoAlvo('jefferson', B, 'local')).toEqual({ serverId: 'local', nome: 'jefferson' });
  });

  it('endereço de outra máquina abre NAQUELA máquina, sem o `::` no nome', () => {
    // O caso do print: o botão dizia "Abrir servidor-b::thread-admin" e montava a rota com o
    // endereço inteiro como nome, no servidor errado.
    expect(rotaDoAlvo('servidor-b::thread-admin', B, 'local'))
      .toEqual({ serverId: 'srv-abc', nome: 'thread-admin' });
  });

  it('casa o servidor pelo id e pelo host, não só pelo rótulo', () => {
    // Três nomes pra mesma máquina: o id do peer é do peers.json do backend, o id do app é do
    // cadastro no aparelho. Não há razão pra coincidirem.
    expect(rotaDoAlvo('srv-abc::manager', B, 'local')?.serverId).toBe('srv-abc');
    expect(rotaDoAlvo('maquina-b::manager', B, 'local')?.serverId).toBe('srv-abc');
  });

  it('nome que não é sessão nenhuma não vira botão', () => {
    // O `to` do SendMessage aceita subagente, que não tem chat no app — era o outro clique morto.
    expect(rotaDoAlvo('revisor-do-diff', B, 'local')).toBeNull();
  });

  it('sessão que não existe NAQUELE servidor não cai no atual', () => {
    // Abrir uma homônima no servidor errado é pior que não abrir.
    expect(rotaDoAlvo('servidor-b::Contas', B, 'local')).toBeNull();
  });

  it('servidor desconhecido no endereço não vira botão', () => {
    expect(rotaDoAlvo('maquina-que-nao-tenho::x', B, 'local')).toBeNull();
  });

  it('lista ainda não carregada não esconde botão — nem o de outra máquina', () => {
    // Achado das revisões: no celular o chat e a lista de sessões são telas EXCLUDENTES, então
    // dentro da conversa o store fica SEM dado — não é borda, é o estado normal ali. Exigir a
    // lista esconderia justo o botão de recado pra outra máquina, que é o que isto veio habilitar.
    // Sem lista não dá pra CONFERIR; esconder seria inventar resposta.
    const frio = [
      { ...LOCAL, sessions: [], loaded: false },
      { ...VPS, sessions: [], loaded: false },
    ] as unknown as ServerBucket[];
    expect(rotaDoAlvo('qualquer', frio, 'local')).toEqual({ serverId: 'local', nome: 'qualquer' });
    expect(rotaDoAlvo('servidor-b::thread-admin', frio, 'local'))
      .toEqual({ serverId: 'srv-abc', nome: 'thread-admin' });
  });

  it('sem NENHUM bucket, o nome cru ainda abre no servidor da rota', () => {
    // Store totalmente vazio (nenhum consumidor montado): o comportamento de antes desta correção.
    expect(rotaDoAlvo('qualquer', [], 'local')).toEqual({ serverId: 'local', nome: 'qualquer' });
    // Mas o endereço de outra máquina continua sem botão: não há como saber qual servidor é.
    expect(rotaDoAlvo('servidor-b::x', [], 'local')).toBeNull();
  });

  it('sem alvo, sem nome depois do `::` ou sem servidor atual: nada', () => {
    expect(rotaDoAlvo('', B, 'local')).toBeNull();
    expect(rotaDoAlvo(null, B, 'local')).toBeNull();
    expect(rotaDoAlvo('servidor-b::', B, 'local')).toBeNull();
    expect(rotaDoAlvo('jefferson', B, null)).toBeNull();
  });
});
