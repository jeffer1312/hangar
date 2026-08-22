import 'react-native-url-polyfill/auto';
import '../src/theme/unistyles';
import { useEffect } from 'react';
import { Stack, useRouter, useSegments } from 'expo-router';
import { GestureHandlerRootView } from 'react-native-gesture-handler';
// Sem o KeyboardProvider, KeyboardStickyView/KeyboardChatScrollView lançam em runtime
import { KeyboardProvider } from 'react-native-keyboard-controller';
import { configureCore } from '../src/net/configureCore';
import { useServers } from '../src/stores/servers';

configureCore();

export default function Layout() {
  const router = useRouter();
  const segments = useSegments();
  const ready = useServers((s) => s.ready);
  const servers = useServers((s) => s.servers);

  useEffect(() => {
    void useServers.getState().load();
  }, []);

  useEffect(() => {
    if (!ready) return;
    const onLogin = segments[0] === 'login';
    if (servers.length === 0 && !onLogin) {
      router.replace('/login');
    }
  }, [ready, servers.length, segments]);

  return (
    <KeyboardProvider>
      <GestureHandlerRootView style={{ flex: 1 }}>
        <Stack screenOptions={{ headerShown: false }} />
      </GestureHandlerRootView>
    </KeyboardProvider>
  );
}
