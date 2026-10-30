import { useEffect, useState } from 'react';
import { listVaultPeers, restoreVault, setVaultRecovery, type Server, type VaultPeer } from '@hangar/core';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { InfoNotice } from '../config/InfoNotice';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { vaultErrorText } from './errors';
import * as m from '../../paraglide/messages';

// O servidor recusa abaixo disso (`erro_cofre_recuperacao_curta`); conferir aqui poupa a ida.
const RECOVERY_MIN = 12;

function useSubmit(onDone: () => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const run = async (work: () => Promise<unknown>, done: string) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try { await work(); toast.ok(done); onDone(); }
    catch (e) { setError(vaultErrorText(e)); haptic.error(); }
    finally { setBusy(false); }
  };
  return { busy, error, run };
}

/** Define ou troca a senha de recuperação; a cópia na outra máquina só abre com ela. */
export function VaultRecoverySheet({ server, onClose, onChanged }: { server: Server; onClose: () => void; onChanged: () => void }) {
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const { busy, error, run } = useSubmit(() => { onChanged(); onClose(); });
  const short = password.length > 0 && password.length < RECOVERY_MIN;
  const mismatch = confirm.length > 0 && confirm !== password;
  const ok = !busy && password.length >= RECOVERY_MIN && confirm === password;

  return <MachineSheet title={m.vault_app_recovery_title()} onClose={onClose}>
    <InfoNotice text={m.vault_app_recovery_warning()} />
    <MachineGroup>
      <MachineField label={m.vault_app_recovery_password()} value={password} onChangeText={setPassword} editable={!busy}
        secureTextEntry autoComplete="off" textContentType="none" />
      <MachineField label={m.vault_app_recovery_confirm()} value={confirm} onChangeText={setConfirm} editable={!busy}
        secureTextEntry autoComplete="off" textContentType="none" />
    </MachineGroup>
    <Notice text={short ? m.vault_app_recovery_short({ min: RECOVERY_MIN }) : mismatch ? m.vault_app_recovery_mismatch() : error} error />
    <MachineGroup>
      <MachineRow title={busy ? m.vault_app_saving() : m.vault_app_save()} action disabled={!ok}
        onPress={() => void run(() => setVaultRecovery(server, password), m.vault_app_recovery_saved())} />
    </MachineGroup>
  </MachineSheet>;
}

/** Restaura num cofre vazio: a cópia vem de `peer`; `ownerId` vazio = a cópia desta máquina. */
export function VaultRestoreSheet({ server, onClose, onChanged }: { server: Server; onClose: () => void; onChanged: () => void }) {
  const [peers, setPeers] = useState<VaultPeer[] | null>(null);
  const [loadError, setLoadError] = useState('');
  const [peer, setPeer] = useState<string | null>(null);
  const [ownerId, setOwnerId] = useState('');
  const [password, setPassword] = useState('');
  const { busy, error, run } = useSubmit(() => { onChanged(); onClose(); });

  useEffect(() => {
    let alive = true;
    listVaultPeers(server)
      .then((list) => { if (alive) setPeers(list); })
      .catch((e: unknown) => { if (alive) setLoadError(vaultErrorText(e)); });
    return () => { alive = false; };
  }, [server]);

  const ok = !busy && !!peer && !!password;
  return <MachineSheet title={m.vault_app_restore_title()} onClose={onClose}>
    <MachineHelp>{m.vault_app_restore_help()}</MachineHelp>
    {loadError ? <Notice text={loadError} error />
      : peers === null ? <InfoNotice text={m.vault_app_peers_loading()} />
      : peers.length === 0 ? <InfoNotice text={m.vault_app_peers_empty({ server: server.label })} />
      : <MachineGroup>
        {peers.map((p) => <MachineRow key={p.id} title={p.id} description={p.base_url}
          selected={p.id === peer} disabled={busy} onPress={() => setPeer(p.id)} />)}
      </MachineGroup>}
    <MachineGroup>
      <MachineField label={m.vault_app_restore_owner()} value={ownerId} onChangeText={setOwnerId} editable={!busy}
        placeholder={m.vault_app_restore_owner_placeholder()} />
      <MachineField label={m.vault_app_recovery_password()} value={password} onChangeText={setPassword} editable={!busy}
        secureTextEntry autoComplete="off" textContentType="none" />
    </MachineGroup>
    <Notice text={error} error />
    <MachineGroup>
      <MachineRow title={busy ? m.vault_app_restoring() : m.vault_app_restore()} action disabled={!ok}
        onPress={() => { if (peer) void run(() => restoreVault(server, password, peer, ownerId.trim() || undefined), m.vault_app_restored()); }} />
    </MachineGroup>
  </MachineSheet>;
}
