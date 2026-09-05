import type { AggSession } from '@hangar/core';

// Os três estados que a lista precisa mostrar, para teste. A lista de produção NÃO importa daqui:
// linha fabricada em tela é sessão que não existe no servidor.
export function sessoesDemo(): AggSession[] {
  const base = { serverId: 's1', serverLabel: 'casa', serverColor: '#8b5cf6', provider: 'claude' as const, tracked: true };
  return [
    { ...base, name: 'api', state: 'idle', cwd: '/home/demo/api', branch: 'main', last_activity: 1_700_000_000 },
    { ...base, name: 'front', state: 'working', cwd: '/home/demo/front', label: 'Editando…', last_activity: 1_700_000_100 },
    { ...base, name: 'infra', state: 'awaiting_input', cwd: '/home/demo/infra', question: 'Deseja continuar?', last_activity: 1_700_000_050 },
  ];
}
