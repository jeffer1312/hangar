import { describe, expect, it } from 'vitest';
import type { AggSession } from './types';
import { filterWorkspaceItems, workspaceSessionItems } from './workspaceCommands';

const items = [
  { key: 'archive', title: 'Arquivo', detail: 'Sessões encerradas', keywords: ['histórico'], group: 'Sessão' as const },
  { key: 'git', title: 'Git', detail: 'Branch e alterações', keywords: ['commit'], group: 'Ferramentas' as const },
];

describe('filterWorkspaceItems', () => {
  it('matches accents and keywords', () => {
    expect(filterWorkspaceItems(items, 'historico').map((item) => item.key)).toEqual(['archive']);
    expect(filterWorkspaceItems(items, 'commit').map((item) => item.key)).toEqual(['git']);
  });

  it('keeps the supplied group order', () => {
    const groupedItems = [
      { key: 'chat', title: 'Conversa', detail: 'Abrir conversa', keywords: [], group: 'Navegação' as const },
      { key: 'archive', title: 'Arquivo', detail: 'Abrir arquivo', keywords: [], group: 'Sessão' as const },
      { key: 'git', title: 'Git', detail: 'Abrir Git', keywords: [], group: 'Ferramentas' as const },
      { key: 'pair', title: 'Parear', detail: 'Parear sessão', keywords: [], group: 'Colaboração' as const },
      { key: 'session', title: 'Projeto', detail: 'Abrir sessão', keywords: [], group: 'Sessões' as const },
    ];

    expect(filterWorkspaceItems(groupedItems, '').map((item) => item.group)).toEqual([
      'Navegação',
      'Sessão',
      'Ferramentas',
      'Colaboração',
      'Sessões',
    ]);
  });

  it('preserves the original order when matches tie', () => {
    const tiedItems = [
      { key: 'first', title: 'Primeiro', detail: 'Abrir projeto', keywords: [], group: 'Sessões' as const },
      { key: 'second', title: 'Segundo', detail: 'Abrir projeto', keywords: [], group: 'Sessões' as const },
    ];

    expect(filterWorkspaceItems(tiedItems, 'projeto').map((item) => item.key)).toEqual(['first', 'second']);
  });
});

describe('workspaceSessionItems', () => {
  it('uses a server-aware key and exposes server/cwd as searchable text', () => {
    const rows: AggSession[] = [
      {
        name: 'api',
        cwd: '/workspace/checkout',
        state: 'idle',
        serverId: 'server-b',
        serverLabel: 'Produção',
        serverColor: '#22c55e',
      },
    ];

    const sessionItems = workspaceSessionItems(rows);

    expect(sessionItems[0]).toMatchObject({
      key: 'server-b::api',
      kind: 'session',
      title: 'api',
      detail: 'Produção · /workspace/checkout',
      group: 'Sessões',
    });
    expect(filterWorkspaceItems(sessionItems, 'producao')).toHaveLength(1);
    expect(filterWorkspaceItems(sessionItems, 'checkout')).toHaveLength(1);
  });
});
