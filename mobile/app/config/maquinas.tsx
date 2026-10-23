import { useEffect, useMemo, useState } from 'react';
import { Stack, useRouter } from 'expo-router';
import type { Server } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { ownServers, useServers } from '../../src/stores/servers';
import { useSessions } from '../../src/stores/sessions';
import { PairAcceptSheet, type PairAcceptTarget } from '../../src/features/pair/PairAcceptSheet';
import { MachineGroup, MachineRow } from '../../src/features/machines/MachineRows';
import { AddMachineSheet } from '../../src/features/machines/AddMachineSheet';
import { UpdateAllMachines } from '../../src/features/machines/UpdateAllMachines';
import * as m from '../../src/paraglide/messages';

export default function Maquinas() {
  const router = useRouter();
  const todos = useServers((s) => s.servers);
  // A entrada do par de fora não é máquina desta pessoa.
  const servers = useMemo(() => ownServers(todos), [todos]);
  const activeId = useServers((s) => s.activeId);
  const ready = useServers((s) => s.ready);
  const [aceite, setAceite] = useState<PairAcceptTarget | null>(null);
  const [adding, setAdding] = useState<{ owner: Server | null } | null>(null);
  const rows = useSessions((s) => s.rows);
  const byServer = useSessions((s) => s.byServer);
  // A mesma lista alimenta os pontos de conexão e a escolha de sessão no par externo.
  useEffect(() => useSessions.getState().retain(), []);
  const alvo = aceite ? { ...aceite, sessions: rows.filter((r) => r.serverId === aceite.server.id) } : null;
  const active = servers.find((s) => s.id === activeId) ?? servers[0] ?? null;
  const parear = () => { if (active) setAceite({ server: active, name: null, sessions: [] }); };

  return (
    <Pagina>
      <Stack.Screen options={{ title: m.maquinas_titulo() }} />
      {!ready ? <InfoNotice text={m.native_server_loading()} /> : servers.length === 0 ? <InfoNotice text={m.maquinas_vazio()} /> : null}
      {servers.length > 0 ? (
        <MachineGroup>
          {servers.map((s) => {
            const bucket = byServer.find((entry) => entry.server.id === s.id);
            const error = bucket?.error;
            return <MachineRow
              key={s.id}
              title={s.label}
              value={error ? error === 'offline' ? m.mobile_machines_offline() : m.mobile_machines_connection_failed() : bucket?.loaded ? m.mobile_machines_connected() : undefined}
              valueTone={error ? 'error' : bucket?.loaded ? 'success' : undefined}
              onPress={() => router.push({ pathname: '/config/maquina', params: { id: s.id } } as never)}
            />;
          })}
        </MachineGroup>
      ) : null}
      <MachineGroup>
        <MachineRow title={m.sessao_adicionar_servidor()} action onPress={() => setAdding({ owner: active })} />
        {servers.length ? <MachineRow title={m.native_par_menu()} onPress={parear} /> : null}
        {ready ? <UpdateAllMachines servers={todos} compact /> : null}
      </MachineGroup>
      {adding ? <AddMachineSheet owner={adding.owner} onClose={() => setAdding(null)} onQr={() => { setAdding(null); router.push('/login' as never); }} /> : null}
      <PairAcceptSheet target={alvo} onClose={() => setAceite(null)} />
    </Pagina>
  );
}
