import { useEffect, useRef, useState } from 'react';
import { saveGuest, type GuestMachine, type Server } from '@hangar/core';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { syncFailureText } from '../login/addressSignIn';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { failureText, thrownText } from './guestTools';
import { closeLegacy, deleteLegacy, legacyPlan, openLegacy, type LegacyAdmin, type LegacySession } from './legacyGuests';
import * as m from '../../paraglide/messages';

/**
 * Contas antigas de uma máquina: o dono digita uma vez a senha da sincronização dela, e cada conta vira conta do
 * Connect com a mesma senha e os mesmos acessos. A antiga só sai depois que a nova foi guardada inteira.
 */
export function LegacySheet({ hub, syncUser, store, machines, mine, onClose, onChanged }: {
  hub: Server; syncUser: string; store: Server; machines: GuestMachine[]; mine: Server[]; onClose: () => void; onChanged: () => void;
}) {
  const [user, setUser] = useState(syncUser);
  const [password, setPassword] = useState('');
  const [session, setSession] = useState<LegacySession | null>(null);
  const [busy, setBusy] = useState(false);
  const [errors, setErrors] = useState<string[]>([]);
  const open = useRef<string | null>(null);
  useEffect(() => () => { if (open.current) void closeLegacy(open.current); }, []);

  const signIn = async () => {
    if (busy || !user.trim() || !password) return;
    setBusy(true);
    setErrors([]);
    try {
      const opened = await openLegacy(hub, user.trim(), password);
      open.current = opened.hub;
      setPassword('');
      setSession(opened);
      haptic.success();
    } catch (error) {
      setErrors([syncFailureText(error)]);
      haptic.error();
    } finally {
      setBusy(false);
    }
  };

  const bring = async (admin: LegacyAdmin) => {
    if (!session || busy) return;
    const plan = legacyPlan(admin, session.ownerServers, mine);
    if ('missing' in plan) {
      setErrors([m.guests_legacy_missing({ servidores: plan.missing.join(', ') })]);
      haptic.error();
      return;
    }
    setBusy(true);
    setErrors([]);
    try {
      const result = await saveGuest(store, machines, plan.draft, plan.prev);
      if (result.errors.length) {
        const labelOf = (id: string) => mine.find((server) => server.id === id)?.label ?? id;
        setErrors(result.errors.map((failure) => failureText(failure, labelOf)));
        haptic.error();
        if (!result.storeFailed) onChanged();
        return;
      }
      await deleteLegacy(session.hub, admin.user);
      setSession({ ...session, guests: session.guests.filter((guest) => guest.user !== admin.user) });
      toast.ok(m.guests_legacy_brought({ usuario: plan.draft.user }));
      onChanged();
    } catch (error) {
      // A conta nova pode ter sido guardada e só a remoção da antiga ter falhado: tentar de novo atualiza os mesmos acessos.
      const code = (error as { code?: string } | null)?.code;
      setErrors([code?.startsWith('sync_') || code === 'network_failed' ? syncFailureText(error) : thrownText(error)]);
      haptic.error();
    } finally {
      setBusy(false);
    }
  };

  return <MachineSheet title={m.guests_legacy_title()} onClose={onClose}>
    {!session ? <>
      <MachineHelp>{m.guests_legacy_login_help({ servidor: hub.label })}</MachineHelp>
      <MachineGroup>
        <MachineField label={m.login_usuario()} value={user} onChangeText={setUser} editable={!busy} textContentType="username" />
        <MachineField label={m.login_senha()} value={password} onChangeText={setPassword} editable={!busy} secureTextEntry
          textContentType="password" returnKeyType="go" onSubmitEditing={() => { void signIn(); }} />
      </MachineGroup>
      {busy ? <MachineHelp>{m.mobile_sync_login_wait()}</MachineHelp> : null}
      {errors.map((text) => <Notice key={text} text={text} error />)}
      <MachineGroup>
        <MachineRow title={busy ? m.login_entrando() : m.login_entrar()} action disabled={busy || !user.trim() || !password} onPress={() => { void signIn(); }} />
      </MachineGroup>
    </> : <>
      <MachineGroup>
        {session.guests.length === 0 ? <MachineRow title={m.guests_legacy_empty()} /> : null}
        {session.guests.map((admin) => <MachineRow key={admin.user} title={admin.user} description={m.guests_legacy_account()}
          value={m.guests_legacy_bring()} disabled={busy} onPress={() => { void bring(admin); }} />)}
      </MachineGroup>
      {errors.map((text) => <Notice key={text} text={text} error />)}
    </>}
  </MachineSheet>;
}
