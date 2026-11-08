import { useEffect, useState } from 'react';
import { RefreshControl } from 'react-native';
import { Stack, useRouter } from 'expo-router';
import { ConnectBoundary } from '../src/features/connect/ConnectBoundary';
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
  const token = useConnect((s) => s.account?.token ?? '');
  const notice = useConnect((s) => s.notice);
  const header = useGlassHeader();
  const [pulling, setPulling] = useState(false);
  useEffect(() => { void useConnect.getState().load(); }, []);
  // Puxar para baixo relê as máquinas e os aparelhos; a roda some quando as duas leituras terminam.
  const refresh = () => {
    setPulling(true);
    void Promise.allSettled([useConnect.getState().refreshMachines(), useConnect.getState().refreshDevices()]).then(() => setPulling(false));
  };
  return <>
    {/* A pilha da raiz nasce sem cabeçalho; esta tela abre sem servidor, fora de `/config`. */}
    <Stack.Screen options={header} />
    <MachinePage title={m.connect_title()} refreshControl={signedIn ? <RefreshControl refreshing={pulling} onRefresh={refresh} /> : undefined}>
      <ConnectBoundary resetKey={token}>
        {!ready ? <Notice text={m.comum_carregando()} /> : null}
        {/* Sem conta, o motivo da volta ao login aparece na abertura dele. */}
        {signedIn ? <Notice text={notice} /> : null}
        {ready ? signedIn ? <ConnectMachines onDone={() => router.replace('/')} /> : <ConnectLogin /> : null}
      </ConnectBoundary>
    </MachinePage>
  </>;
}
