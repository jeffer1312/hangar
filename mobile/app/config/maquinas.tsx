import { useEffect, useMemo, useState } from 'react';
import { View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { getConfigForServer, type Server } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader, Pill } from '../../src/features/config/PageHeader';
import { SectionCard } from '../../src/features/config/SectionCard';
import { SettingsRow } from '../../src/features/config/SettingsRow';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { ownServers, useServers } from '../../src/stores/servers';
import { useSessions } from '../../src/stores/sessions';
import { PairAcceptSheet, type PairAcceptTarget } from '../../src/features/pair/PairAcceptSheet';
import { MachineSettingsSheet, useForgetMachine } from '../../src/features/machines/MachineSettingsSheet';
import { AddMachineSheet } from '../../src/features/machines/AddMachineSheet';
import { UpdateAllMachines } from '../../src/features/machines/UpdateAllMachines';
import { ConnectionDot, Notice, useMachineAction } from '../../src/features/machines/parts';
import * as m from '../../src/paraglide/messages';

export default function Maquinas() {
  const router = useRouter();
  const todos = useServers((s) => s.servers);
  // A entrada do par de fora não é máquina desta pessoa.
  const servers = useMemo(() => ownServers(todos), [todos]);
  const activeId = useServers((s) => s.activeId);
  const ready = useServers((s) => s.ready);
  const [aceite, setAceite] = useState<PairAcceptTarget | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [adding, setAdding] = useState<{ owner: Server | null } | null>(null);
  const editing = servers.find((s) => s.id === editingId) ?? null;
  const rows = useSessions((s) => s.rows);
  const byServer = useSessions((s) => s.byServer);
  // A mesma lista alimenta os pontos de conexão e a escolha de sessão no par externo.
  useEffect(() => useSessions.getState().retain(), []);
  useEffect(() => { setEditingId(null); }, [activeId]);
  const remover = useForgetMachine(() => setEditingId(null));
  const alvo = aceite ? { ...aceite, sessions: rows.filter((r) => r.serverId === aceite.server.id) } : null;
  const active = servers.find((s) => s.id === activeId) ?? servers[0] ?? null;
  const parear = () => { if (active) setAceite({ server: active, name: null, sessions: [] }); };

  return (
    <Pagina>
      <PageHeader
        title={m.maquinas_titulo()}
        subtitle={m.mobile_machines_list_hint()}
        actions={[
          { icon: 'Plus', label: m.sessao_adicionar_servidor(), onPress: () => setAdding({ owner: active }) },
          ...(servers.length ? [{ icon: 'Link' as const, label: m.native_par_menu(), onPress: parear }] : []),
        ]}
      />
      {ready ? <UpdateAllMachines servers={todos} /> : null}
      {!ready ? <InfoNotice text={m.native_server_loading()} /> : servers.length === 0 ? <InfoNotice text={m.maquinas_vazio()} /> : null}
      {servers.length > 0 ? (
        <SectionCard icon="Server" title={m.native_machines_others()}>
          {servers.map((s) => {
            const bucket = byServer.find((entry) => entry.server.id === s.id);
            const error = bucket?.error;
            return <SettingsRow
              key={s.id}
              icon="Monitor"
              title={s.label}
              description={s.baseUrl}
              onPress={() => setEditingId(s.id)}
              onLongPress={() => remover(s)}
              right={<ConnectionDot label={s.label} loaded={bucket?.loaded ?? false} error={error} />}
            >
              <View accessibilityRole="alert" accessibilityLiveRegion="polite">
                {error ? <InfoNotice text={error === 'offline' ? m.mobile_machines_offline() : error} /> : null}
              </View>
              <View style={styles.pills}>
                <Pill icon="Settings" label={m.mobile_machines_configure()} onPress={() => setEditingId(s.id)} />
                <TestConnection key={`${s.id}:${s.baseUrl}:${s.token}`} server={s} />
              </View>
            </SettingsRow>;
          })}
        </SectionCard>
      ) : null}
      {editing ? <MachineSettingsSheet key={`${editing.id}:${editing.baseUrl}:${editing.token}`} server={editing} onClose={() => setEditingId(null)} /> : null}
      {adding ? <AddMachineSheet owner={adding.owner} onClose={() => setAdding(null)} onQr={() => { setAdding(null); router.push('/login' as never); }} /> : null}
      <PairAcceptSheet target={alvo} onClose={() => setAceite(null)} />
    </Pagina>
  );
}

function TestConnection({ server }: { server: Server }) {
  const action = useMachineAction();
  return <View>
    <Pill icon="Plug" label={action.busy ? m.mobile_machines_busy() : m.config_maquinas_testar()} disabled={action.busy}
      onPress={() => action.run(() => getConfigForServer(server, 15_000), () => {}, m.config_maquinas_conexao_ok())} />
    <Notice text={action.error} error /><Notice text={action.message} />
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  pills: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2] },
}));
