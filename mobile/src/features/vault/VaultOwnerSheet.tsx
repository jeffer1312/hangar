import { useEffect, useMemo, useState } from 'react';
import { getVaultStatus, putVaultConfig, type Server } from '@hangar/core';
import { MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { InfoNotice } from '../config/InfoNotice';
import { ownServers, useServers } from '../../stores/servers';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { vaultErrorText } from './errors';
import * as m from '../../paraglide/messages';

type Candidate = { server: Server; ownerId: string };

/** Escolhe, entre as outras máquinas, a que guarda o cofre: só quem responde `role: owner` com o próprio id. */
export function VaultOwnerSheet({ server, current, onClose, onChanged }: {
  server: Server;
  current?: string;
  onClose: () => void;
  onChanged: () => void;
}) {
  const all = useServers((s) => s.servers);
  const others = useMemo(() => ownServers(all).filter((s) => s.id !== server.id && !s.disabled), [all, server.id]);
  const [found, setFound] = useState<Candidate[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let alive = true;
    setFound(null);
    void Promise.allSettled(others.map(async (s) => ({ s, st: await getVaultStatus(s) }))).then((results) => {
      if (!alive) return;
      setFound(results.flatMap((r) => r.status === 'fulfilled' && r.value.st.role === 'owner' && r.value.st.server_id
        ? [{ server: r.value.s, ownerId: r.value.st.server_id }] : []));
    });
    return () => { alive = false; };
  }, [others]);

  const choose = async (ownerId: string) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      await putVaultConfig(server, { owner: ownerId });
      toast.ok(m.vault_app_owner_saved());
      onChanged();
      onClose();
    } catch (e) { setError(vaultErrorText(e)); haptic.error(); }
    finally { setBusy(false); }
  };

  return <MachineSheet title={m.vault_app_owner_choose()} onClose={onClose}>
    <MachineHelp>{m.vault_app_owner_choose_help({ server: server.label })}</MachineHelp>
    {found === null ? <InfoNotice text={m.vault_app_owner_searching()} />
      : found.length === 0 ? <InfoNotice text={m.vault_app_owner_none()} />
      : <MachineGroup>
        {found.map((f) => <MachineRow key={f.server.id} title={f.server.label} value={f.ownerId}
          selected={f.ownerId === current} disabled={busy} onPress={() => void choose(f.ownerId)} />)}
      </MachineGroup>}
    <Notice text={error} error />
  </MachineSheet>;
}
