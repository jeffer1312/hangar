import { useEffect } from 'react';
import { useLocalSearchParams, useRouter } from 'expo-router';
import { ownServers, useServers } from '../../src/stores/servers';
import { useSessions } from '../../src/stores/sessions';
import { MachineSettings } from '../../src/features/machines/MachineSettingsSheet';
import type { MachineView } from '../../src/features/machines/MachineRows';
import { MachinePage } from '../../src/features/machines/MachinePage';
import { Notice } from '../../src/features/machines/parts';
import * as m from '../../src/paraglide/messages';

const views: MachineView[] = ['edit', 'connection', 'discover', 'remote', 'advanced', 'identity', 'terminal', 'diagnostics', 'peers', 'maintenance'];

export default function MachineRoute() {
  const params = useLocalSearchParams<{ id?: string; view?: string }>();
  const router = useRouter();
  const servers = useServers((s) => s.servers);
  const server = ownServers(servers).find((s) => s.id === params.id);
  const view = views.find((value) => value === params.view) ?? 'edit';
  useEffect(() => useSessions.getState().retain(), []);
  if (!server) return <MachinePage title={m.maquinas_titulo()}><Notice text={m.mobile_machines_entry_changed()} error /></MachinePage>;
  return <MachineSettings key={`${server.id}:${server.baseUrl}:${server.token}`} server={server} view={view}
    onNavigate={(next) => router.push({ pathname: '/config/maquina', params: { id: server.id, view: next } } as never)}
    onClose={() => router.dismissTo('/config/maquinas' as never)} />;
}
