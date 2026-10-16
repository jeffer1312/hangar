import { useCallback, useMemo, useRef, useState } from 'react';
import { BackHandler, Keyboard, PanResponder, Platform, Text, View } from 'react-native';
import { Pressable } from 'react-native-gesture-handler';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useFocusEffect, useLocalSearchParams, useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { DrawerLayoutMethods } from 'react-native-gesture-handler/ReanimatedDrawerLayout';
import { Screen } from '../src/ui/Screen';
import { Icon } from '../src/ui/Icon';
import { SessionsDrawer } from '../src/features/sessions/SessionsDrawer';
import { SessionList } from '../src/features/sessions/SessionList';
import { ServerSheet } from '../src/features/sessions/ServerSheet';
import { CreateSessionSheet } from '../src/features/create/CreateSessionSheet';
import * as m from '../src/paraglide/messages';
import { LiveVoiceChatButton } from '../src/features/voice/LiveVoiceChatButton';
import { useServers } from '../src/stores/servers';
import { useSessions } from '../src/stores/sessions';
import { useAparencia, type HomeScreen } from '../src/stores/aparencia';

export default function Index() {
  const router = useRouter();
  const params = useLocalSearchParams<{ compose?: string | string[] }>();
  const compose = Array.isArray(params.compose) ? params.compose[0] : params.compose;
  const activeId = useServers((s) => s.activeId);
  const count = useSessions((s) => s.rows.filter((row) => row.state !== 'dead').length);
  const { theme } = useUnistyles();
  const insets = useSafeAreaInsets();
  const drawer = useRef<DrawerLayoutMethods>(null);
  const [mode, setMode] = useState<HomeScreen>(() => useAparencia.getState().homeScreen);
  const [focusRequest, setFocusRequest] = useState(() => mode === 'newConversation' ? 1 : 0);
  const [serversOpen, setServersOpen] = useState(false);
  const closeDrawer = useCallback(() => drawer.current?.closeDrawer(), []);
  const showSessions = useCallback(() => { Keyboard.dismiss(); setMode('sessions'); }, []);
  const openConversation = useCallback((focus: boolean) => {
    setMode('newConversation');
    if (focus) setFocusRequest((request) => request + 1);
  }, []);
  const newFromDrawer = useCallback(() => { closeDrawer(); openConversation(true); }, [closeDrawer, openConversation]);
  // O keyboardDismissMode da gaveta só age no arrasto; pelo botão o campo seguiria focado atrás dela.
  const openDrawer = () => { Keyboard.dismiss(); drawer.current?.openDrawer(); };

  useFocusEffect(useCallback(() => {
    if (compose !== '1') return;
    openConversation(true);
    router.setParams({ compose: undefined });
  }, [compose, openConversation, router]));

  useFocusEffect(useCallback(() => {
    if (Platform.OS !== 'android') return;
    const sub = BackHandler.addEventListener('hardwareBackPress', () => {
      if (mode !== 'newConversation') return false;
      showSessions();
      return true;
    });
    return () => sub.remove();
  }, [mode, showSessions]));

  // Só o cabeçalho inicia o gesto: lista, campo e controles de uso conservam os seus movimentos.
  const swipe = useMemo(() => PanResponder.create({
    onMoveShouldSetPanResponder: (_event, gesture) => mode === 'newConversation' && gesture.dy > 12 && gesture.dy > Math.abs(gesture.dx) * 2,
    onPanResponderRelease: (_event, gesture) => { if (gesture.dy > 40) showSessions(); },
  }), [mode, showSessions]);

  // Pressable do gesture-handler recebe o toque mesmo na faixa de arrasto da gaveta no Android.
  const topButton = (icon: 'Server' | 'Settings', label: string, onPress: () => void) => (
    <Pressable onPress={onPress} style={styles.icon} accessibilityRole="button" accessibilityLabel={label} hitSlop={4}>
      <Icon name={icon} size={20} color={theme.tokens.text.secondary} />
    </Pressable>
  );

  return (
    <Screen edges={[]}>
      <SessionsDrawer ref={drawer} onClose={closeDrawer} onNewConversation={newFromDrawer} onOpenServers={() => setServersOpen(true)}>
        <View style={styles.bar} {...swipe.panHandlers}>
          <Pressable onPress={mode === 'newConversation' ? showSessions : openDrawer} style={styles.sessions}
            accessibilityRole="button" accessibilityLabel={m.native_sessions()} accessibilityValue={{ text: String(count) }}>
            <Icon name={mode === 'newConversation' ? 'ChevronLeft' : 'PanelLeft'} size={22} color={theme.tokens.text.primary} />
            <Text style={styles.sessionsLabel}>{m.native_sessions()}</Text>
            <View style={styles.badge}><Text style={styles.count}>{count}</Text></View>
          </Pressable>
          <View style={styles.spacer} />
          {activeId ? <LiveVoiceChatButton serverId={activeId} /> : null}
          {topButton('Server', m.maquinas_este_aparelho(), () => setServersOpen(true))}
          {topButton('Settings', m.config_modal_titulo(), () => router.push('/config' as never))}
        </View>
        <CreateSessionSheet keyboardOffset={Platform.OS === 'ios' ? insets.top + BAR : undefined}
          home={{ mode, focusRequest, onNewConversation: openConversation,
            sessions: <SessionList variant="home" onClose={closeDrawer} onOpenServers={() => setServersOpen(true)} /> }} />
      </SessionsDrawer>
      <ServerSheet open={serversOpen} onFechar={() => setServersOpen(false)} />
    </Screen>
  );
}

const BAR = 44;
const styles = StyleSheet.create((theme) => ({
  bar: { flexDirection: 'row', alignItems: 'center', paddingHorizontal: theme.base.space[1] },
  spacer: { flex: 1 },
  icon: { width: BAR, height: BAR, alignItems: 'center', justifyContent: 'center' },
  sessions: { minHeight: BAR, minWidth: BAR, flexDirection: 'row', alignItems: 'center', gap: 8, paddingHorizontal: 8, flexShrink: 1 },
  sessionsLabel: { fontSize: theme.base.text.base, fontWeight: '600', color: theme.tokens.text.primary, flexShrink: 1 },
  badge: { minWidth: 22, minHeight: 22, borderRadius: 11, paddingHorizontal: 6, alignItems: 'center', justifyContent: 'center', backgroundColor: theme.tokens.bg.hover },
  count: { fontSize: theme.base.text.xs, fontWeight: '600', color: theme.tokens.text.primary, fontVariant: ['tabular-nums'] },
}));
