import { useEffect, useRef, useState } from 'react';
import { Alert, AppState, Text } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import * as Clipboard from 'expo-clipboard';
import { addVaultEntry, deleteVaultEntry, revealVaultPassword, updateVaultEntry, type Server, type VaultEntry, type VaultEntryChange, type VaultPolicy } from '@hangar/core';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { useSettingsColors } from '../config/colors';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { vaultErrorText } from './errors';
import * as m from '../../paraglide/messages';

export function PolicyRows({ value, onChange, disabled }: { value: VaultPolicy; onChange: (p: VaultPolicy) => void; disabled?: boolean }) {
  return <>
    <MachineRow title={m.vault_app_policy_ask()} description={m.vault_app_policy_ask_help()} selected={value === 'ask'}
      disabled={disabled} onPress={() => onChange('ask')} />
    <MachineRow title={m.vault_app_policy_free()} description={m.vault_app_policy_free_help()} selected={value === 'free'}
      disabled={disabled} onPress={() => onChange('free')} />
  </>;
}

/** `null` em `entry` cadastra uma nova; a senha só sai do servidor ao tocar em Mostrar ou Copiar. */
export function VaultEntrySheet({ server, entry, onClose, onChanged }: {
  server: Server;
  entry: VaultEntry | null;
  onClose: () => void;
  onChanged: () => void;
}) {
  const c = useSettingsColors();
  const [name, setName] = useState(entry?.name ?? '');
  const [username, setUsername] = useState(entry?.username ?? '');
  const [url, setUrl] = useState(entry?.url ?? '');
  const [notes, setNotes] = useState(entry?.notes ?? '');
  const [password, setPassword] = useState('');
  const [policy, setPolicy] = useState<VaultPolicy>(entry?.policy ?? 'ask');
  const [shown, setShown] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  // Resposta que chega depois de o app ir para o fundo não pode pôr a senha de volta na tela.
  const revealGeneration = useRef(0);

  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') return;
      revealGeneration.current++;
      setShown(null);
    });
    return () => { revealGeneration.current++; sub.remove(); };
  }, []);

  const run = async (work: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try { await work(); }
    catch (e) { setError(vaultErrorText(e)); haptic.error(); }
    finally { setBusy(false); }
  };

  const trimmed = name.trim();
  const canSave = !busy && !!trimmed && (entry ? true : !!password);
  const opt = (v: string, old?: string) => (v.trim() === (old ?? '') ? undefined : v.trim() || null);
  const save = () => run(async () => {
    if (entry) {
      const change: VaultEntryChange = {
        name: trimmed === entry.name ? undefined : trimmed,
        username: opt(username, entry.username),
        url: opt(url, entry.url),
        notes: opt(notes, entry.notes),
        password: password || undefined,
        policy: policy === entry.policy ? undefined : policy,
      };
      await updateVaultEntry(server, entry.id, change);
    } else {
      await addVaultEntry(server, { name: trimmed, username: username.trim() || null, password, url: url.trim() || null, notes: notes.trim() || null, policy });
    }
    setPassword('');
    toast.ok(m.vault_app_saved());
    onChanged();
    onClose();
  });

  const reveal = () => {
    if (shown !== null) { setShown(null); return; }
    if (!entry) return;
    const generation = revealGeneration.current;
    void run(async () => {
      const { password } = await revealVaultPassword(server, entry.id);
      if (generation === revealGeneration.current && AppState.currentState === 'active') setShown(password);
    });
  };
  const copy = () => {
    if (entry) void run(async () => {
      await Clipboard.setStringAsync((await revealVaultPassword(server, entry.id)).password);
      toast.ok(m.toast_copiado());
    });
  };
  const remove = () => {
    if (!entry) return;
    Alert.alert(m.vault_app_delete_confirm({ name: entry.name }), m.vault_app_delete_confirm_body(), [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.vault_app_delete(), style: 'destructive', onPress: () => void run(async () => {
        await deleteVaultEntry(server, entry.id);
        onChanged();
        onClose();
      }) },
    ]);
  };

  return <MachineSheet title={entry ? entry.name : m.vault_app_add()} onClose={onClose}>
    <MachineHelp>{m.vault_token_hint()}</MachineHelp>
    <MachineGroup>
      <MachineField label={m.vault_app_field_name()} value={name} onChangeText={setName} editable={!busy} />
      <MachineField label={m.vault_app_field_username()} value={username} onChangeText={setUsername} editable={!busy}
        autoComplete="off" textContentType="none" />
      <MachineField label={m.vault_app_field_site()} value={url} onChangeText={setUrl} editable={!busy} keyboardType="url" />
      <MachineField label={entry ? m.vault_app_field_new_password() : m.vault_app_field_password()} value={password}
        onChangeText={setPassword} editable={!busy} secureTextEntry autoComplete="off" textContentType="none"
        placeholder={entry ? m.vault_app_field_new_password_placeholder() : undefined} />
      <MachineField label={m.vault_app_field_notes()} value={notes} onChangeText={setNotes} editable={!busy} stacked multiline />
    </MachineGroup>
    <MachineGroup>
      <PolicyRows value={policy} onChange={setPolicy} disabled={busy} />
    </MachineGroup>
    {entry ? <MachineGroup>
      <MachineRow title={shown !== null ? m.vault_app_hide() : m.vault_app_reveal()} action disabled={busy} onPress={reveal} />
      {shown !== null ? <Text style={[styles.secret, { color: c.text }]}>{shown}</Text> : null}
      <MachineRow title={m.vault_app_copy()} action disabled={busy} onPress={copy} />
    </MachineGroup> : null}
    <Notice text={error} error />
    <MachineGroup>
      <MachineRow title={busy ? m.vault_app_saving() : m.vault_app_save()} action disabled={!canSave} onPress={save} />
      {entry ? <MachineRow title={m.vault_app_delete()} danger disabled={busy} onPress={remove} /> : null}
    </MachineGroup>
  </MachineSheet>;
}

const styles = StyleSheet.create((theme) => ({
  secret: { paddingHorizontal: theme.base.space[4], paddingBottom: theme.base.space[3], fontFamily: theme.base.fontMono, fontSize: theme.base.text.sm },
}));
