import type { ConnectDevice } from '../machines/api';

export type GroupTarget = { serverId: string; label: string; deviceId: string };
export type PendingGroup = { key: string; name: string; code: string; createdAt: number | null; targets: GroupTarget[] };
export type MachineDevices = { serverId: string; label: string; rows: ConnectDevice[] | null };

/** `off` guarda máquinas (`serverId`), não pedidos: nova tentativa do celular troca o pedido, não a escolha. Sobrando
 *  uma máquina só não há caixa para remarcar: ela vale marcada. */
export function chosenTargets(group: PendingGroup, off: ReadonlySet<string>): GroupTarget[] {
  if (group.targets.length === 1) return group.targets;
  return group.targets.filter((target) => !off.has(target.serverId));
}

/** Um cartão por celular pendente. Código e chave de grupo vêm prontos do servidor; o nome é de quem pede, e nome
 *  diferente nunca junta. Sem chave (servidor ou app antigo), cada pedido fica no seu. Nova tentativa do celular
 *  substitui a anterior no servidor: cada máquina tem um pedido só dele. Cartão igual ao de `before` volta o mesmo objeto. */
export function pendingGroups(machines: MachineDevices[], before: PendingGroup[] = []): PendingGroup[] {
  const groups = new Map<string, PendingGroup>();
  for (const { serverId, label, rows } of machines) {
    for (const row of rows ?? []) {
      if (row.status !== 'pending') continue;
      const key = row.install_group ? `${row.install_group}\n${row.name}` : `${serverId}:${row.device_id}`;
      const group = groups.get(key) ?? { key, name: row.name, code: row.install_code ?? '', createdAt: null, targets: [] };
      groups.set(key, group);
      group.targets.push({ serverId, label, deviceId: row.device_id });
      // Servidor antigo não manda `created_at`: sem idade, o cartão não mostra "pediu há".
      if (row.created_at) group.createdAt = Math.min(group.createdAt ?? row.created_at, row.created_at);
    }
  }
  return [...groups.values()].map((group) => {
    const old = before.find((candidate) => candidate.key === group.key);
    return old && JSON.stringify(old) === JSON.stringify(group) ? old : group;
  });
}
