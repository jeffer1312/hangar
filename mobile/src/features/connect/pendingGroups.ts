import type { ConnectDevice } from '../machines/api';

export type GroupTarget = { serverId: string; label: string; deviceId: string };
export type PendingGroup = { key: string; name: string; targets: GroupTarget[] };
export type MachineDevices = { serverId: string; label: string; rows: ConnectDevice[] | null };

/** Um cartão por celular pendente; sem `install_id` (app antigo), cada pedido fica no seu. */
export function pendingGroups(machines: MachineDevices[]): PendingGroup[] {
  const groups = new Map<string, PendingGroup>();
  for (const { serverId, label, rows } of machines) {
    for (const row of rows ?? []) {
      if (row.status !== 'pending') continue;
      const key = row.install_id ?? `${serverId}:${row.device_id}`;
      const group = groups.get(key) ?? { key, name: row.name, targets: [] };
      group.targets.push({ serverId, label, deviceId: row.device_id });
      groups.set(key, group);
    }
  }
  return [...groups.values()];
}
