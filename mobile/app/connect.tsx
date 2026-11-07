import { useEffect } from 'react';
import { Stack, useRouter } from 'expo-router';
import { ConnectLogin } from '../src/features/connect/ConnectLogin';
import { ConnectMachines } from '../src/features/connect/ConnectMachines';
import { useConnect } from '../src/features/connect/connectStore';
import { MachinePage } from '../src/features/machines/MachinePage';
import { Notice } from '../src/features/machines/parts';
import { useGlassHeader } from '../src/ui/glassHeader';
import * as m from '../src/paraglide/messages';

export default function ConnectRoute() {
  const router = useRouter();
  const ready = useConnect((s) => s.ready);
  const signedIn = useConnect((s) => !!s.account);
  const notice = useConnect((s) => s.notice);
  const header = useGlassHeader();
  useEffect(() => { void useConnect.getState().load(); }, []);
  return <>
    {/* A pilha da raiz nasce sem cabeçalho; esta tela abre sem servidor, fora de `/config`. */}
    <Stack.Screen options={header} />
    <MachinePage title={m.connect_title()}>
      {!ready ? <Notice text={m.comum_carregando()} /> : null}
      <Notice text={notice} />
      {ready ? signedIn ? <ConnectMachines onDone={() => router.replace('/')} /> : <ConnectLogin /> : null}
    </MachinePage>
  </>;
}
