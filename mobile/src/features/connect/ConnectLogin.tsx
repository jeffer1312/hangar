import { useState } from 'react';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { Notice, useMachineAction } from '../machines/parts';
import { useConnect } from './connectStore';
import * as m from '../../paraglide/messages';

export function ConnectLogin() {
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState('');
  const [password, setPassword] = useState('');
  const action = useMachineAction();
  const submit = () => {
    const sent = password;
    setPassword('');
    void action.run(() => (creating ? useConnect.getState().signUp(name, sent) : useConnect.getState().signIn(name, sent)), () => {});
  };
  const label = creating ? m.connect_sign_up() : m.login_entrar();
  return <>
    <MachineHelp>{m.connect_login_help()}</MachineHelp>
    <MachineGroup>
      <MachineField label={m.connect_account_name()} value={name} onChangeText={setName} editable={!action.busy}
        textContentType="username" autoComplete="username" stacked />
      <MachineField label={m.login_senha()} value={password} onChangeText={setPassword} editable={!action.busy} secureTextEntry
        textContentType={creating ? 'newPassword' : 'password'} autoComplete={creating ? 'new-password' : 'current-password'} stacked />
    </MachineGroup>
    {creating ? <MachineHelp>{m.connect_account_rules()}</MachineHelp> : null}
    <Notice text={action.error} error />
    <MachineGroup>
      <MachineRow title={action.busy ? m.mobile_machines_busy() : label} action disabled={action.busy || !name.trim() || !password} onPress={submit} />
      <MachineRow title={creating ? m.connect_have_account() : m.connect_no_account()} disabled={action.busy} onPress={() => setCreating(!creating)} />
    </MachineGroup>
  </>;
}
