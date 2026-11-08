import { installCode } from '@hangar/core';
import type { ConnectDevice } from '../machines/api';

/** `stale`: pedidos anteriores do mesmo celular nesta máquina (tentou de novo antes de expirar); saem junto com a resposta. */
export type GroupTarget = { serverId: string; label: string; deviceId: string; stale: string[] };
export type PendingGroup = { key: string; name: string; code: string; createdAt: number | null; targets: GroupTarget[] };
export type MachineDevices = { serverId: string; label: string; rows: ConnectDevice[] | null };

/** `off` guarda máquinas (`serverId`), não pedidos: nova tentativa do celular troca o pedido, não a escolha. */
export function chosenTargets(group: PendingGroup, off: ReadonlySet<string>): GroupTarget[] {
  return group.targets.filter((target) => !off.has(target.serverId));
}

/** Um cartão por celular pendente, uma caixa por máquina. O nome é de quem pede: nome diferente nunca junta, mesmo com o
 *  mesmo id. Sem `install_id` (app antigo), cada pedido fica no seu. Cartão igual ao de `before` volta o mesmo objeto. */
export function pendingGroups(machines: MachineDevices[], before: PendingGroup[] = []): PendingGroup[] {
  const groups = new Map<string, PendingGroup>();
  const newest = new Map<GroupTarget, number>();
  for (const { serverId, label, rows } of machines) {
    for (const row of rows ?? []) {
      if (row.status !== 'pending') continue;
      const key = row.install_id ? `${row.install_id}\n${row.name}` : `${serverId}:${row.device_id}`;
      const group = groups.get(key) ?? { key, name: row.name, code: row.install_id ? installCode(row.install_id) : '', createdAt: null, targets: [] };
      groups.set(key, group);
      const at = row.created_at ?? 0;
      const same = group.targets.find((target) => target.serverId === serverId);
      if (!same) {
        const target = { serverId, label, deviceId: row.device_id, stale: [] };
        group.targets.push(target);
        newest.set(target, at);
      } else if (at >= (newest.get(same) ?? 0)) {
        same.stale.push(same.deviceId);
        same.deviceId = row.device_id;
        newest.set(same, at);
      } else same.stale.push(row.device_id);
    }
  }
  return [...groups.values()].map((group) => {
    // Servidor antigo não manda `created_at` (0 aqui): sem idade, o cartão não mostra "pediu há".
    group.createdAt = Math.min(...group.targets.map((target) => newest.get(target) ?? 0)) || null;
    const old = before.find((candidate) => candidate.key === group.key);
    return old && JSON.stringify(old) === JSON.stringify(group) ? old : group;
  });
}
