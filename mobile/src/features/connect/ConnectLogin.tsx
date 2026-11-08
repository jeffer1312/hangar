import { useState } from 'react';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { Notice, useMachineAction } from '../machines/parts';
import { useConnect } from './connectStore';
import { ConnectHero } from './ConnectHero';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

export function ConnectLogin() {
  // `null` é a abertura; escolhido, o formulário de criar conta ou de entrar.
  const [mode, setMode] = useState<'create' | 'login' | null>(null);
  const [name, setName] = useState('');
  const [password, setPassword] = useState('');
  // O erro é do modo em que foi pedido: trocar entre entrar e criar conta começa limpo.
  const [round, setRound] = useState(0);
  const [sentIn, setSentIn] = useState(-1);
  // Por que a pessoa voltou ao login (sessão encerrada, cofre): aparece na abertura e no formulário.
  const notice = useConnect((s) => s.notice);
  const action = useMachineAction();
  const creating = mode === 'create';
  const ready = !action.busy && !!name.trim() && !!password;
  const choose = (next: 'create' | 'login' | null) => { setMode(next); setRound(round + 1); };
  const submit = () => {
    if (!ready) return;
    setSentIn(round);
    // A senha só sai do campo quando a conta abriu: errada, a pessoa corrige em vez de redigitar.
    void action.run(async () => {
      try { await (creating ? useConnect.getState().signUp(name, password) : useConnect.getState().signIn(name, password)); }
      catch (error) { haptic.error(); throw error; }
      haptic.success();
    }, () => setPassword(''));
  };
  if (!mode) return <ConnectHero notice={notice} onCreate={() => choose('create')} onLogin={() => choose('login')} />;
  const label = creating ? m.connect_sign_up() : m.login_entrar();
  return <>
    <Notice text={notice} error />
    <MachineHelp>{m.connect_login_help()}</MachineHelp>
    <MachineGroup>
      <MachineField label={m.connect_account_name()} value={name} onChangeText={setName} editable={!action.busy}
        textContentType="username" autoComplete="username" returnKeyType="next" stacked />
      <MachineField label={m.login_senha()} value={password} onChangeText={setPassword} editable={!action.busy} secureTextEntry
        textContentType={creating ? 'newPassword' : 'password'} autoComplete={creating ? 'new-password' : 'current-password'}
        returnKeyType="go" onSubmitEditing={submit} stacked />
    </MachineGroup>
    {creating ? <MachineHelp>{m.connect_account_rules()}</MachineHelp> : null}
    <Notice text={sentIn === round ? action.error : ''} error />
    <MachineGroup>
      <MachineRow title={action.busy ? m.mobile_machines_busy() : label} action disabled={!ready} onPress={submit} />
      <MachineRow title={creating ? m.connect_have_account() : m.connect_no_account()} disabled={action.busy}
        onPress={() => choose(creating ? 'login' : 'create')} />
      {/* Voltar à abertura não deixa a senha esperando no campo escondido. */}
      <MachineRow title={m.comum_voltar()} disabled={action.busy} onPress={() => { setPassword(''); choose(null); }} />
    </MachineGroup>
  </>;
}
