import { useEffect, useRef, useState } from 'react';
import { normalizeGuestUser, openLegacyGuests, saveGuest, suggestGuestUser, validGuestUser, type GuestMachine,
  type LegacySession, type Server } from '@hangar/core';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { syncFailureText } from '../login/addressSignIn';
import { syncCrypto } from '../login/syncCrypto';
import { useConnect } from '../connect/connectStore';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { failureText, thrownText } from './guestTools';
import * as m from '../../paraglide/messages';

type Renaming = { user: string; name: string };

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
  const [renaming, setRenaming] = useState<Renaming | null>(null);
  const owner = useConnect((s) => s.account?.name) || syncUser;
  const open = useRef<LegacySession | null>(null);
  const mounted = useRef(true);
  // O estado `busy` só chega no próximo render: dois toques seguidos passariam os dois.
  const inFlight = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; void open.current?.close(); };
  }, []);

  const signIn = async () => {
    if (inFlight.current || !user.trim() || !password) return;
    inFlight.current = true;
    setBusy(true);
    setErrors([]);
    try {
      const opened = await openLegacyGuests(hub, user.trim(), password, syncCrypto);
      // Folha fechada durante a entrada: ninguém mais fecharia esta sessão.
      if (!mounted.current) { void opened.close(); return; }
      open.current = opened;
      setPassword('');
      setSession(opened);
      haptic.success();
    } catch (error) {
      if (!mounted.current) return;
      setErrors([syncFailureText(error)]);
      haptic.error();
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(false);
    }
  };

  // Nome sem `.` nem `@` não serve no Connect: a pessoa escolhe o novo antes de qualquer acesso mudar.
  const pick = (legacy: string) => {
    setErrors([]);
    if (validGuestUser(normalizeGuestUser(legacy))) void bring(legacy, legacy);
    else setRenaming({ user: legacy, name: suggestGuestUser(legacy, owner) });
  };

  const bring = async (legacy: string, name: string) => {
    if (!session || inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setErrors([]);
    let saved = false;
    try {
      // A senha e os acessos da conta só são abertos aqui, e só desta conta.
      const plan = session.plan(legacy, mine, name);
      if ('missing' in plan) {
        setErrors([m.guests_legacy_missing({ servidores: plan.missing.join(', ') })]);
        haptic.error();
        return;
      }
      const result = await saveGuest(store, machines, plan.draft, plan.prev);
      if (result.errors.length) {
        const labelOf = (id: string) => mine.find((server) => server.id === id)?.label ?? id;
        setErrors(result.errors.map((failure) => failureText(failure, labelOf)));
        haptic.error();
        if (!result.storeFailed) onChanged();
        return;
      }
      saved = true;
      await session.remove(legacy);
      setRenaming(null);
      setSession({ ...session, users: session.users.filter((guest) => guest !== legacy) });
      toast.ok(m.guests_legacy_brought({ usuario: plan.draft.user }));
      onChanged();
    } catch (error) {
      // A conta nova pode ter sido guardada e só a remoção da antiga ter falhado: a lista já mostra a nova, e tentar de
      // novo atualiza os mesmos acessos.
      if (saved) onChanged();
      const code = (error as { code?: string } | null)?.code;
      setErrors([code?.startsWith('sync_') || code === 'network_failed' ? syncFailureText(error) : thrownText(error)]);
      haptic.error();
    } finally {
      inFlight.current = false;
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
    </> : renaming ? <>
      <MachineHelp>{m.guests_legacy_rename_help({
        antigo: renaming.user, novo: normalizeGuestUser(renaming.name) || renaming.name })}</MachineHelp>
      <MachineGroup>
        <MachineField label={m.guests_legacy_new_name()} value={renaming.name} editable={!busy} textContentType="username" autoComplete="off"
          onChangeText={(name) => setRenaming({ ...renaming, name })} returnKeyType="go"
          onSubmitEditing={() => { void bring(renaming.user, renaming.name); }} />
      </MachineGroup>
      {errors.map((text) => <Notice key={text} text={text} error />)}
      <MachineGroup>
        <MachineRow title={busy ? m.convidados_salvando() : m.guests_legacy_bring()} action disabled={busy || !renaming.name.trim()}
          onPress={() => { void bring(renaming.user, renaming.name); }} />
        <MachineRow title={m.comum_cancelar()} action disabled={busy} onPress={() => { setRenaming(null); setErrors([]); }} />
      </MachineGroup>
    </> : <>
      <MachineGroup>
        {session.users.length === 0 ? <MachineRow title={m.guests_legacy_empty()} /> : null}
        {session.users.map((legacy) => <MachineRow key={legacy} title={legacy} description={m.guests_legacy_account()}
          value={m.guests_legacy_bring()} disabled={busy} onPress={() => pick(legacy)} />)}
      </MachineGroup>
      {errors.map((text) => <Notice key={text} text={text} error />)}
    </>}
  </MachineSheet>;
}
