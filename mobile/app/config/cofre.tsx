import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert } from 'react-native';
import { Stack } from 'expo-router';
import { findOwnerServer, getVaultStatus, listVaultEntries, listVaultPeers, putVaultConfig, type Server, type VaultEntry, type VaultPeer, type VaultState, type VaultStatus } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { VaultEntrySheet } from '../../src/features/vault/VaultEntrySheet';
import { VaultPeerSheet } from '../../src/features/vault/VaultPeerSheet';
import { VaultRecoverySheet, VaultRestoreSheet } from '../../src/features/vault/VaultBackupSheets';
import { vaultErrorText } from '../../src/features/vault/errors';
import { useServers } from '../../src/stores/servers';
import { toast } from '../../src/ui/Toast';
import { haptic } from '../../src/ui/haptics';
import * as m from '../../src/paraglide/messages';

const STATE: Record<VaultState, () => string> = {
  open: m.vault_app_state_open,
  locked: m.vault_app_state_locked,
  absent: m.vault_app_state_absent,
  offline: m.vault_app_state_offline,
  key_lost: m.vault_app_state_key_lost,
};

const host = (url?: string) => {
  if (!url) return '';
  try { return new URL(url).host || url; } catch { return url; }
};

export default function Cofre() {
  const server = useServers((s) => s.active());
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [entries, setEntries] = useState<VaultEntry[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<VaultEntry | 'new' | null>(null);
  const [sheet, setSheet] = useState<'owner' | 'backup' | 'recovery' | 'restore' | null>(null);
  // Quem recebe gravar, revelar e apagar: a própria máquina na dona, o servidor salvo da dona na remota.
  const [writer, setWriter] = useState<Server | null>(null);
  const [ownerLookupError, setOwnerLookupError] = useState('');
  const generation = useRef(0);
  // A lista republica o objeto do servidor sem mudar nada; recarregar por isso fecharia o formulário aberto.
  const serverRef = useRef(server);
  serverRef.current = server;

  const load = useCallback(() => {
    const now = ++generation.current;
    const current = serverRef.current;
    if (!current) return;
    setLoading(true);
    setError('');
    void (async () => {
      try {
        const next = await getVaultStatus(current);
        // Servidor antigo sem `owner_url`: o endereço sai dos peers. Falha aqui deixa a página só de leitura, dizendo por quê.
        const needPeers = next.role === 'remote' && !next.owner_url && !!next.owner;
        let lookupError = '';
        const [list, peers] = await Promise.all([
          next.state === 'open' && next.role !== 'none' ? listVaultEntries(current) : null,
          needPeers ? listVaultPeers(current).catch((e: unknown): VaultPeer[] => { lookupError = vaultErrorText(e); return []; }) : ([] as VaultPeer[]),
        ]);
        const ownerUrl = next.owner_url ?? peers.find((p) => p.id === next.owner)?.base_url;
        const owner = next.role === 'owner' ? current
          : next.role === 'remote' && ownerUrl ? findOwnerServer(ownerUrl, useServers.getState().servers) : null;
        if (now !== generation.current) return;
        setStatus(next);
        setEntries(list);
        setWriter(owner);
        setOwnerLookupError(lookupError);
      } catch (e) {
        if (now === generation.current) setError(vaultErrorText(e));
      } finally {
        if (now === generation.current) setLoading(false);
      }
    })();
  }, []);

  useEffect(() => {
    setStatus(null);
    setEntries(null);
    setWriter(null);
    setOwnerLookupError('');
    setEditing(null);
    setSheet(null);
    load();
    return () => { generation.current++; };
  }, [load, server?.id, server?.baseUrl, server?.token]);

  const becomeOwner = () => {
    if (!server) return;
    Alert.alert(m.vault_app_owner_self_confirm({ server: server.label }), m.vault_app_owner_self_confirm_body(), [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.vault_app_owner_self(), onPress: () => {
        setBusy(true);
        setError('');
        putVaultConfig(server, { owner: 'self' })
          .then(() => { toast.ok(m.vault_app_owner_saved()); load(); })
          .catch((e: unknown) => { setError(vaultErrorText(e)); haptic.error(); })
          .finally(() => setBusy(false));
      } },
    ]);
  };

  const state = status?.state;
  const backup = status?.backup;
  // Restaurar só nesta máquina: sem cofre ainda, dona aberta sem nenhuma senha, ou dona com a chave perdida
  // (o servidor põe o arquivo velho de lado e cria chave nova).
  const canRestore = !!status && (status.role === 'none'
    || (status.role === 'owner' && (state === 'key_lost' || (state === 'open' && entries?.length === 0))));
  return (
    <Pagina>
      <Stack.Screen options={{ title: m.vault_app_title() }} />
      {!server ? <InfoNotice text={m.vault_app_no_server()} />
        : !status && loading ? <InfoNotice text={m.vault_app_loading()} />
        : !status && error ? <>
          <Notice text={error} error />
          <MachineGroup><MachineRow title={m.vault_app_retry()} action onPress={load} /></MachineGroup>
        </>
        : status ? <>
          <MachineHelp>{m.vault_app_where()}</MachineHelp>
          <MachineGroup>
            {status.role === 'owner' ? <MachineRow title={m.vault_app_owner_self()} value={state ? STATE[state]() : undefined}
              valueTone={state === 'open' ? 'success' : 'error'} />
              : status.role === 'remote' ? <>
                <MachineRow title={m.vault_app_owner_is({ owner: status.owner ?? '?' })} value={state ? STATE[state]() : undefined}
                  valueTone={state === 'open' ? 'success' : 'error'} />
                <MachineRow title={m.vault_app_owner_change()} disabled={busy} onPress={() => setSheet('owner')} />
              </>
              : <>
                <MachineRow title={m.vault_app_owner_self()} description={m.vault_app_owner_self_help()} action disabled={busy} onPress={becomeOwner} />
                <MachineRow title={m.vault_app_owner_other()} disabled={busy} onPress={() => setSheet('owner')} />
              </>}
          </MachineGroup>
          {status.role === 'none' ? <InfoNotice text={m.vault_app_none_help()} />
            : state === 'locked' ? <InfoNotice text={m.vault_app_locked_help()} />
            : state === 'key_lost' ? <Notice text={m.vault_app_key_lost_help()} error />
            : state === 'offline' ? <Notice text={status.detail ? `${m.vault_app_offline_help()} ${status.detail}` : m.vault_app_offline_help()} error />
            : state === 'absent' ? <InfoNotice text={status.detail ?? m.vault_app_absent_help()} />
            : null}
          <Notice text={error} error />
          {state === 'open' && status.role !== 'none' ? <>
            <MachineHelp>{m.vault_app_entries()}</MachineHelp>
            {entries && entries.length === 0 ? <InfoNotice text={m.vault_app_empty()} /> : null}
            {entries?.length ? <MachineGroup>
              {entries.map((e) => <MachineRow key={e.id} title={e.name}
                description={[e.username, host(e.url)].filter(Boolean).join(' · ') || undefined}
                value={e.policy === 'free' ? m.vault_app_policy_free() : m.vault_app_policy_ask()}
                onPress={writer ? () => setEditing(e) : undefined} />)}
            </MachineGroup> : null}
            {!writer ? ownerLookupError ? <Notice text={m.vault_app_read_only_failed({ error: ownerLookupError })} error />
              : <InfoNotice text={m.vault_app_read_only({ owner: status.owner ?? '?' })} /> : null}
            <MachineGroup>
              {writer ? <MachineRow title={m.vault_app_add()} action onPress={() => setEditing('new')} /> : null}
              <MachineRow title={m.vault_app_retry()} disabled={loading} onPress={load} />
            </MachineGroup>
          </> : <MachineGroup><MachineRow title={m.vault_app_retry()} disabled={loading} onPress={load} /></MachineGroup>}
          {status.role === 'owner' ? <>
            <MachineHelp>{m.vault_app_backup()}</MachineHelp>
            <MachineGroup>
              <MachineRow title={m.vault_app_backup_peer()} value={backup?.peer ?? m.vault_app_backup_none()}
                disabled={busy} onPress={() => setSheet('backup')} />
              <MachineRow title={m.vault_app_recovery_title()} description={m.vault_app_recovery_help()}
                disabled={busy} onPress={() => setSheet('recovery')} />
              {backup ? <MachineRow title={m.vault_app_backup_last()}
                value={backup.last_ok_at ? new Date(backup.last_ok_at * 1000).toLocaleString() : m.vault_app_backup_never()}
                valueTone={backup.last_error ? 'error' : backup.last_ok_at ? 'success' : undefined} /> : null}
            </MachineGroup>
            {backup?.last_error ? <Notice text={m.vault_app_backup_error({ error: backup.last_error })} error /> : null}
          </> : null}
          {canRestore ? <MachineGroup>
            <MachineRow title={m.vault_app_restore()} description={m.vault_app_restore_row_help()} disabled={busy} onPress={() => setSheet('restore')} />
          </MachineGroup> : null}
        </> : null}
      {writer && editing ? <VaultEntrySheet server={writer} entry={editing === 'new' ? null : editing}
        onClose={() => setEditing(null)} onChanged={load} /> : null}
      {server && sheet === 'owner' ? <VaultPeerSheet server={server} title={m.vault_app_owner_choose()}
        help={m.vault_app_owner_choose_help({ server: server.label })} current={status?.owner}
        onPick={async (id) => { if (id) { await putVaultConfig(server, { owner: id }); toast.ok(m.vault_app_owner_saved()); load(); } }}
        onClose={() => setSheet(null)} /> : null}
      {server && sheet === 'backup' ? <VaultPeerSheet server={server} title={m.vault_app_backup_peer()}
        help={m.vault_app_backup_peer_help()} current={backup?.peer ?? null} noneLabel={m.vault_app_backup_none()}
        onPick={async (id) => { await putVaultConfig(server, { backup_peer: id }); toast.ok(m.vault_app_backup_saved()); load(); }}
        onClose={() => setSheet(null)} /> : null}
      {server && sheet === 'recovery' ? <VaultRecoverySheet server={server} onClose={() => setSheet(null)} onChanged={load} /> : null}
      {server && sheet === 'restore' ? <VaultRestoreSheet server={server} onClose={() => setSheet(null)} onChanged={load} /> : null}
    </Pagina>
  );
}
