import { useRef } from 'react';
import { View } from 'react-native';
import { BlurTargetView } from 'expo-blur';
import { SafeAreaView } from 'react-native-safe-area-context';
import { StyleSheet } from 'react-native-unistyles';
import { Background } from './Background';
import { BlurTargetContext } from './blurTarget';

export function Screen({ children }: { children: React.ReactNode }) {
  // O alvo do blur do Android: o Glass lá dentro pega esta ref pelo contexto.
  const alvo = useRef<View>(null);
  return (
    <BlurTargetContext.Provider value={alvo}>
      <BlurTargetView ref={alvo} style={styles.root}>
        <Background />
        <SafeAreaView style={styles.safe} edges={['top', 'bottom']}>
          {children}
        </SafeAreaView>
      </BlurTargetView>
    </BlurTargetContext.Provider>
  );
}

// Fundo transparente: quem pinta é o Background, e ele já começa com a cor opaca do tema.
const styles = StyleSheet.create(() => ({
  root: { flex: 1, backgroundColor: 'transparent' },
  safe: { flex: 1 },
}));
