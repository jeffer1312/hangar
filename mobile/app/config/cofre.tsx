import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert } from 'react-native';
import { Stack } from 'expo-router';
import { getVaultStatus, listVaultEntries, putVaultConfig, type VaultEntry, type VaultState, type VaultStatus } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { VaultEntrySheet } from '../../src/features/vault/VaultEntrySheet';
import { VaultOwnerSheet } from '../../src/features/vault/VaultOwnerSheet';
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
  const [choosing, setChoosing] = useState(false);
  const generation = useRef(0);

  const load = useCallback(() => {
    const now = ++generation.current;
    if (!server) return;
    setLoading(true);
    setError('');
    void (async () => {
      try {
        const next = await getVaultStatus(server);
        const list = next.state === 'open' && next.role !== 'none' ? await listVaultEntries(server) : null;
        if (now !== generation.current) return;
        setStatus(next);
        setEntries(list);
      } catch (e) {
        if (now === generation.current) setError(vaultErrorText(e));
      } finally {
        if (now === generation.current) setLoading(false);
      }
    })();
  }, [server]);

  useEffect(() => {
    setStatus(null);
    setEntries(null);
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
                <MachineRow title={m.vault_app_owner_change()} disabled={busy} onPress={() => setChoosing(true)} />
              </>
              : <>
                <MachineRow title={m.vault_app_owner_self()} description={m.vault_app_owner_self_help()} action disabled={busy} onPress={becomeOwner} />
                <MachineRow title={m.vault_app_owner_other()} disabled={busy} onPress={() => setChoosing(true)} />
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
                onPress={() => setEditing(e)} />)}
            </MachineGroup> : null}
            <MachineGroup>
              <MachineRow title={m.vault_app_add()} action onPress={() => setEditing('new')} />
              <MachineRow title={m.vault_app_retry()} disabled={loading} onPress={load} />
            </MachineGroup>
          </> : <MachineGroup><MachineRow title={m.vault_app_retry()} disabled={loading} onPress={load} /></MachineGroup>}
        </> : null}
      {server && editing ? <VaultEntrySheet server={server} entry={editing === 'new' ? null : editing}
        onClose={() => setEditing(null)} onChanged={load} /> : null}
      {server && choosing ? <VaultOwnerSheet server={server} current={status?.owner}
        onClose={() => setChoosing(false)} onChanged={load} /> : null}
    </Pagina>
  );
}
