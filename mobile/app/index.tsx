import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { BackHandler, Keyboard, PanResponder, Platform, Text, View } from 'react-native';
import { Pressable } from 'react-native-gesture-handler';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useFocusEffect, useIsFocused, useLocalSearchParams, useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { DrawerLayoutMethods } from 'react-native-gesture-handler/ReanimatedDrawerLayout';
import { Screen } from '../src/ui/Screen';
import { useTabletLandscape } from '../src/features/tablet/layout';
import { WorkingMarkPaused } from '../src/ui/WorkingMark';
import { Icon } from '../src/ui/Icon';
import { EdgeButton } from '../src/ui/EdgeButton';
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
  const focused = useIsFocused();
  const tablet = useTabletLandscape();
  const params = useLocalSearchParams<{ compose?: string | string[]; hidden?: string | string[] }>();
  const compose = Array.isArray(params.compose) ? params.compose[0] : params.compose;
  // "Ver sessões ocultas" da página Sessões: a lista precisa estar à vista, com a seção já aberta.
  const hidden = Array.isArray(params.hidden) ? params.hidden[0] : params.hidden;
  const activeId = useServers((s) => s.activeId);
  const count = useSessions((s) => s.rows.filter((row) => row.state !== 'dead').length);
  // O contador lê a lista também com a gaveta fechada e a tela de nova conversa à frente.
  useEffect(() => useSessions.getState().retain(), []);
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
  // O keyboardDismissMode da gaveta só age no arrasto; pelo botão o campo seguiria focado atrás dela.
  const openDrawer = () => { Keyboard.dismiss(); drawer.current?.openDrawer(); };

  useFocusEffect(useCallback(() => {
    if (compose !== '1') return;
    openConversation(true);
    router.setParams({ compose: undefined });
  }, [compose, openConversation, router]));

  useFocusEffect(useCallback(() => {
    if (hidden !== '1') return;
    showSessions();
    router.setParams({ hidden: undefined });
  }, [hidden, showSessions, router]));

  useFocusEffect(useCallback(() => {
    if (tablet || Platform.OS !== 'android') return;
    const sub = BackHandler.addEventListener('hardwareBackPress', () => {
      if (mode !== 'newConversation') return false;
      showSessions();
      return true;
    });
    return () => sub.remove();
  }, [tablet, mode, showSessions]));

  // Só o cabeçalho inicia o gesto: lista, campo e controles de uso conservam os seus movimentos.
  const swipe = useMemo(() => PanResponder.create({
    onMoveShouldSetPanResponder: (_event, gesture) => mode === 'newConversation' && gesture.dy > 12 && gesture.dy > Math.abs(gesture.dx) * 2,
    onPanResponderRelease: (_event, gesture) => { if (gesture.dy > 40) showSessions(); },
  }), [mode, showSessions]);

  const topButton = (icon: 'Server' | 'Settings' | 'ChartNoAxesCombined', label: string, onPress: () => void) => (
    <Pressable onPress={onPress} style={styles.icon} accessibilityRole="button" accessibilityLabel={label} hitSlop={4}>
      <Icon name={icon} size={20} color={theme.tokens.text.secondary} />
    </Pressable>
  );

  // Com o chat por cima, as marcas desta tela param.
  return (
    <WorkingMarkPaused.Provider value={!focused}>
    <Screen edges={[]}>
      <SessionsDrawer ref={drawer} enabled={!tablet} openOnBack={mode === 'sessions'} onClose={closeDrawer} onOpenServers={() => setServersOpen(true)}>
        {!tablet ? <View style={styles.bar} {...swipe.panHandlers}>
          <EdgeButton onPress={mode === 'newConversation' ? showSessions : openDrawer} style={styles.sessions}
            accessibilityLabel={m.native_sessions()} accessibilityValue={{ text: String(count) }}>
            <Icon name={mode === 'newConversation' ? 'ChevronLeft' : 'PanelLeft'} size={22} color={theme.tokens.text.primary} />
            <Text style={styles.sessionsLabel}>{m.native_sessions()}</Text>
            <View style={styles.badge}><Text style={styles.count}>{count}</Text></View>
          </EdgeButton>
          <View style={styles.spacer} />
          {activeId ? <LiveVoiceChatButton serverId={activeId} /> : null}
          {topButton('ChartNoAxesCombined', m.nav_custos(), () => router.push('/costs' as never))}
          {topButton('Server', m.maquinas_este_aparelho(), () => setServersOpen(true))}
          {topButton('Settings', m.config_modal_titulo(), () => router.push('/config' as never))}
        </View> : null}
        <CreateSessionSheet keyboardOffset={!tablet && Platform.OS === 'ios' ? insets.top + BAR : undefined}
          home={{ mode: tablet ? 'newConversation' : mode, focusRequest, onNewConversation: openConversation,
            sessions: tablet ? null : <SessionList variant="home" onClose={closeDrawer} onOpenServers={() => setServersOpen(true)} /> }} />
      </SessionsDrawer>
      <ServerSheet open={serversOpen} onFechar={() => setServersOpen(false)} />
    </Screen>
    </WorkingMarkPaused.Provider>
  );
}

const BAR = 44;
const styles = StyleSheet.create((theme) => ({
  bar: { flexDirection: 'row', alignItems: 'center', paddingHorizontal: theme.base.space[1] },
  spacer: { flex: 1 },
  icon: { width: BAR, height: BAR, alignItems: 'center', justifyContent: 'center' },
  sessions: { minHeight: BAR, minWidth: BAR, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[2], flexShrink: 1 },
  sessionsLabel: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary, flexShrink: 1 },
  badge: { minWidth: 22, minHeight: 22, borderRadius: theme.base.radius.full, paddingHorizontal: theme.base.space[1.5], alignItems: 'center', justifyContent: 'center', backgroundColor: theme.tokens.bg.hover },
  count: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary, fontVariant: ['tabular-nums'] },
}));
