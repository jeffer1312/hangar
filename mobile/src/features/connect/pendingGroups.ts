import type { ConnectDevice } from '../machines/api';

export type GroupTarget = { serverId: string; label: string; deviceId: string };
export type PendingGroup = { key: string; name: string; code: string; createdAt: number | null; targets: GroupTarget[] };
export type MachineDevices = { serverId: string; label: string; rows: ConnectDevice[] | null };

/** O que o celular mostra e o cartão repete, para o dono conferir que é o mesmo aparelho antes de aprovar. */
export const installCode = (installId: string) => installId.slice(0, 4).toUpperCase();

/** Um cartão por celular pendente. O nome é de quem pede: nome diferente nunca junta, mesmo com o mesmo id.
 *  Sem `install_id` (app antigo), cada pedido fica no seu. */
export function pendingGroups(machines: MachineDevices[]): PendingGroup[] {
  const groups = new Map<string, PendingGroup>();
  for (const { serverId, label, rows } of machines) {
    for (const row of rows ?? []) {
      if (row.status !== 'pending') continue;
      const key = row.install_id ? `${row.install_id}\n${row.name}` : `${serverId}:${row.device_id}`;
      const group = groups.get(key) ?? { key, name: row.name, code: row.install_id ? installCode(row.install_id) : '', createdAt: null, targets: [] };
      if (row.created_at !== undefined) group.createdAt = Math.min(group.createdAt ?? row.created_at, row.created_at);
      group.targets.push({ serverId, label, deviceId: row.device_id });
      groups.set(key, group);
    }
  }
  return [...groups.values()];
}
