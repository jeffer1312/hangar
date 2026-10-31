import { useEffect, useState } from 'react';
import { listVaultPeers, type Server, type VaultPeer } from '@hangar/core';
import { MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { InfoNotice } from '../config/InfoNotice';
import { haptic } from '../../ui/haptics';
import { vaultErrorText } from './errors';
import * as m from '../../paraglide/messages';

/** Lista as máquinas do `peers.json` de `server`; `onPick(null)` só aparece com `noneLabel`. */
export function VaultPeerSheet({ server, title, help, current, noneLabel, onPick, onClose }: {
  server: Server;
  title: string;
  help: string;
  current?: string | null;
  noneLabel?: string;
  onPick: (id: string | null) => Promise<void>;
  onClose: () => void;
}) {
  const [peers, setPeers] = useState<VaultPeer[] | null>(null);
  const [loadError, setLoadError] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let alive = true;
    listVaultPeers(server)
      .then((list) => { if (alive) setPeers(list); })
      .catch((e: unknown) => { if (alive) setLoadError(vaultErrorText(e)); });
    return () => { alive = false; };
  }, [server]);

  const pick = async (id: string | null) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try { await onPick(id); onClose(); }
    catch (e) { setError(vaultErrorText(e)); haptic.error(); }
    finally { setBusy(false); }
  };

  return <MachineSheet title={title} onClose={onClose}>
    <MachineHelp>{help}</MachineHelp>
    {loadError ? <Notice text={loadError} error />
      : peers === null ? <InfoNotice text={m.vault_app_peers_loading()} />
      : peers.length === 0 && !noneLabel ? <InfoNotice text={m.vault_app_peers_empty({ server: server.label })} />
      : <MachineGroup>
        {peers.map((p) => <MachineRow key={p.id} title={p.id} description={p.base_url}
          selected={p.id === current} disabled={busy} onPress={() => void pick(p.id)} />)}
        {noneLabel ? <MachineRow title={noneLabel} selected={!current} disabled={busy} onPress={() => void pick(null)} /> : null}
      </MachineGroup>}
    <Notice text={error} error />
  </MachineSheet>;
}
