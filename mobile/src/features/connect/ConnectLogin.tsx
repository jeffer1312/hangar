import { useState } from 'react';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { Notice, useMachineAction } from '../machines/parts';
import { useConnect } from './connectStore';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

export function ConnectLogin() {
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState('');
  const [password, setPassword] = useState('');
  // O erro é do modo em que foi pedido: trocar entre entrar e criar conta começa limpo.
  const [mode, setMode] = useState(0);
  const [sentIn, setSentIn] = useState(-1);
  const action = useMachineAction();
  const ready = !action.busy && !!name.trim() && !!password;
  const submit = () => {
    if (!ready) return;
    setSentIn(mode);
    // A senha só sai do campo quando a conta abriu: errada, a pessoa corrige em vez de redigitar.
    void action.run(async () => {
      try { await (creating ? useConnect.getState().signUp(name, password) : useConnect.getState().signIn(name, password)); }
      catch (error) { haptic.error(); throw error; }
      haptic.success();
    }, () => setPassword(''));
  };
  const label = creating ? m.connect_sign_up() : m.login_entrar();
  return <>
    <MachineHelp>{m.connect_login_help()}</MachineHelp>
    <MachineGroup>
      <MachineField label={m.connect_account_name()} value={name} onChangeText={setName} editable={!action.busy}
        textContentType="username" autoComplete="username" returnKeyType="next" stacked />
      <MachineField label={m.login_senha()} value={password} onChangeText={setPassword} editable={!action.busy} secureTextEntry
        textContentType={creating ? 'newPassword' : 'password'} autoComplete={creating ? 'new-password' : 'current-password'}
        returnKeyType="go" onSubmitEditing={submit} stacked />
    </MachineGroup>
    {creating ? <MachineHelp>{m.connect_account_rules()}</MachineHelp> : null}
    <Notice text={sentIn === mode ? action.error : ''} error />
    <MachineGroup>
      <MachineRow title={action.busy ? m.mobile_machines_busy() : label} action disabled={!ready} onPress={submit} />
      <MachineRow title={creating ? m.connect_have_account() : m.connect_no_account()} disabled={action.busy}
        onPress={() => { setCreating(!creating); setMode(mode + 1); }} />
    </MachineGroup>
  </>;
}
