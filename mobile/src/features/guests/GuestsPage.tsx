import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Stack } from 'expo-router';
import { getSyncSetupForServer, listGuests, type GuestAccount, type Server } from '@hangar/core';
import { Pagina } from '../config/Pagina';
import { InfoNotice } from '../config/InfoNotice';
import { SectionCard } from '../config/SectionCard';
import { MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { Notice } from '../machines/parts';
import { useConnect } from '../connect/connectStore';
import { ownServers, useServers } from '../../stores/servers';
import { GuestSheet } from './GuestSheet';
import { LegacySheet } from './LegacySheet';
import { guestMachines, guestSummary } from './guestTools';
import * as m from '../../paraglide/messages';

type Legacy = { server: Server; user: string };

/** Configurações › Convidados: as contas ficam no servidor escolhido, e cada uma entra pelo Connect. */
export function GuestsPage() {
  const active = useServers((s) => s.active());
  const servers = useServers((s) => s.servers);
  const accountMachines = useConnect((s) => s.machines);
  const store = active && !active.invite ? active : null;
  const machines = useMemo(() => guestMachines(servers, accountMachines), [servers, accountMachines]);
  const mine = useMemo(() => ownServers(servers).filter((s) => !s.disabled), [servers]);
  const [guests, setGuests] = useState<GuestAccount[] | null>(null);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [editing, setEditing] = useState<GuestAccount | 'new' | null>(null);
  const [legacy, setLegacy] = useState<Legacy[]>([]);
  const [opened, setOpened] = useState<Legacy | null>(null);
  const generation = useRef(0);
  const storeRef = useRef(store);
  storeRef.current = store;

  const load = useCallback(() => {
    const current = storeRef.current;
    const now = ++generation.current;
    if (!current) return;
    setLoading(true);
    setError('');
    listGuests(current).then((rows) => {
      if (now === generation.current) setGuests(rows);
    }, (e: unknown) => {
      if (now !== generation.current) return;
      const status = (e as { status?: number } | null)?.status;
      setError(status === 404 ? m.guests_update_server() : m.convidados_erro({ erro: e instanceof Error ? e.message : String(e) }));
    }).finally(() => { if (now === generation.current) setLoading(false); });
  }, []);

  useEffect(() => {
    setGuests(null);
    setEditing(null);
    load();
    return () => { generation.current++; };
  }, [load, store?.id, store?.baseUrl, store?.token]);

  // O host do Connect de cada máquina vem da conta; sem conta, o cofre do convidado leva o endereço que este aparelho usa.
  useEffect(() => {
    void useConnect.getState().load().then(() => useConnect.getState().refreshMachines());
  }, []);

  // Só aparece a máquina com sincronização registrada: é nela que o cofre antigo pode ter contas de convidado.
  const ids = mine.map((s) => s.id).join('|');
  useEffect(() => {
    let alive = true;
    void Promise.all(mine.map((server) => getSyncSetupForServer(server)
      .then((setup) => (setup.enabled && setup.registered && setup.user ? [{ server, user: setup.user }] : []), () => [])))
      .then((rows) => { if (alive) setLegacy(rows.flat()); });
    return () => { alive = false; };
    // Só a troca de máquinas relê; o objeto de cada servidor muda a cada evento da lista.
  }, [ids]);

  return <Pagina>
    <Stack.Screen options={{ title: m.convidados_titulo() }} />
    {!store ? <InfoNotice text={m.guests_no_server()} />
      : !guests && loading ? <InfoNotice text={m.comum_carregando()} />
      : !guests && error ? <>
        <Notice text={error} error />
        <MachineGroup><MachineRow title={m.mobile_machines_retry()} action onPress={load} /></MachineGroup>
      </>
      : guests ? <>
        <MachineHelp>{m.guests_connect_description()}</MachineHelp>
        <MachineGroup>
          {guests.length === 0 ? <MachineRow title={m.convidados_vazio()} /> : null}
          {guests.map((guest) => <MachineRow key={guest.user} title={guest.user}
            description={guest.nameTaken ? m.guests_name_taken() : guestSummary(guest)} valueTone={guest.nameTaken ? 'error' : undefined}
            onPress={() => setEditing(guest)} />)}
          <MachineRow title={m.convidados_adicionar()} action onPress={() => setEditing('new')} />
        </MachineGroup>
        {/* Lista já na tela e releitura falhou: o aviso diz que ela pode estar velha. */}
        <Notice text={error} error />
        {legacy.length ? <SectionCard icon="History" title={m.guests_legacy_title()}>
          {legacy.map((row) => <MachineRow key={row.server.id} title={row.server.label}
            description={m.guests_legacy_server_help({ usuario: row.user })} onPress={() => setOpened(row)} />)}
        </SectionCard> : null}
      </> : null}
    {store && editing ? <GuestSheet store={store} machines={machines} guest={editing === 'new' ? null : editing}
      onClose={() => setEditing(null)} onChanged={load} /> : null}
    {store && opened ? <LegacySheet hub={opened.server} syncUser={opened.user} store={store} machines={machines} mine={mine}
      onClose={() => setOpened(null)} onChanged={load} /> : null}
  </Pagina>;
}
