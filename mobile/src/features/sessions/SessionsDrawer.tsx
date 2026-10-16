import type { ReactNode, Ref } from 'react';
import { useWindowDimensions } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import Animated, { interpolate, useAnimatedStyle, useReducedMotion, type SharedValue } from 'react-native-reanimated';
import { SafeAreaView } from 'react-native-safe-area-context';
import ReanimatedDrawerLayout, {
  DrawerKeyboardDismissMode,
  DrawerType,
  type DrawerLayoutMethods,
} from 'react-native-gesture-handler/ReanimatedDrawerLayout';
import { Glass } from '../../ui/Glass';
import { SessionList } from './SessionList';

// Gaveta de sessões da tela inicial e do chat. Ocupa a altura toda (passa por baixo da barra de
// status, como Claude/ChatGPT): quem usa monta numa `Screen edges={[]}`, e a margem segura fica só
// no conteúdo, aqui dentro. `onClose` roda antes de toda navegação que a lista faz.
export function SessionsDrawer({ ref, onClose, onNewConversation, onOpenServers, children }: {
  ref?: Ref<DrawerLayoutMethods>;
  onClose: () => void;
  onNewConversation?: () => void;
  onOpenServers: () => void;
  children: ReactNode;
}) {
  const { width } = useWindowDimensions();
  return (
    <ReanimatedDrawerLayout
      ref={ref}
      drawerType={DrawerType.FRONT}
      drawerWidth={Math.min(width * 0.85, 360)}
      edgeWidth={32}
      overlayColor="rgba(0,0,0,0.3)"
      animationSpeed={0.8}
      keyboardDismissMode={DrawerKeyboardDismissMode.ON_DRAG}
      renderNavigationView={(progress) => (
        <DrawerPanel progress={progress}>
          <SessionList onClose={onClose} onNewConversation={onNewConversation} onOpenServers={onOpenServers} />
        </DrawerPanel>
      )}
    >
      {(progress) => <DrawerContent progress={progress}>{children}</DrawerContent>}
    </ReanimatedDrawerLayout>
  );
}

// O conteúdo recua um pouco junto com o arrasto; com Reduzir movimento fica parado (o escurecido basta).
function DrawerContent({ progress, children }: { progress?: SharedValue<number>; children: ReactNode }) {
  const reduced = useReducedMotion();
  const anim = useAnimatedStyle(() => ({
    transform: [{ translateX: reduced || !progress ? 0 : interpolate(progress.value, [0, 1], [0, 36]) }],
  }));
  return (
    <Animated.View style={[styles.content, anim]}>
      <SafeAreaView style={styles.content}>{children}</SafeAreaView>
    </Animated.View>
  );
}

// A sombra acompanha a abertura: fixa, ela vazaria na borda esquerda com a gaveta fechada.
function DrawerPanel({ progress, children }: { progress: SharedValue<number>; children: ReactNode }) {
  const shadow = useAnimatedStyle(() => ({
    boxShadow: `0px 0px 28px rgba(0,0,0,${0.35 * progress.value})`,
  }));
  return (
    <Animated.View style={[styles.drawerShadow, shadow]}>
      <Glass variant="panel" style={styles.drawer}>{children}</Glass>
    </Animated.View>
  );
}

const styles = StyleSheet.create(() => ({
  content: { flex: 1, backgroundColor: 'transparent' },
  drawerShadow: { flex: 1 },
  // Altura toda e colada na borda: só o fio do lado que fica sobre a tela.
  drawer: { flex: 1, borderRadius: 0, borderWidth: 0, borderRightWidth: StyleSheet.hairlineWidth },
}));
