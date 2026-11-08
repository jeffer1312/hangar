import type { ReactElement, ReactNode, RefObject } from 'react';
import { Platform, ScrollView, type RefreshControlProps } from 'react-native';
import { useHeaderHeight } from 'expo-router/react-navigation';
import { StyleSheet } from 'react-native-unistyles';
import type { Edge } from 'react-native-safe-area-context';
import { Screen } from '../../ui/Screen';

// O cabeçalho das telas de config é transparente (o papel de parede passa por trás do título) e,
// no Android, não tem fundo. Lá a rolagem começa na borda de baixo dele e corta ali, então o texto
// dos cards nunca passa sob o título. A altura é a medida: no formSheet do Android ela já inclui o recuo
// da barra de status, e por isso a borda de cima da área segura fica de fora.
const BORDAS_SEM_TOPO: Edge[] = ['bottom', 'left', 'right'];
// No iOS o recuo de cima e de baixo é do próprio ScrollView (`automatic`): o conteúdo rola por
// baixo do cabeçalho e o desfoque dele tem o que desfocar. A borda de baixo daqui dobraria o recuo.
const BORDAS_IOS: Edge[] = ['left', 'right'];
const IOS = Platform.OS === 'ios';

/** Casca das telas de `/config`: papel de parede, área segura e rolagem com o recuo do cabeçalho. */
export function Pagina({
  children,
  scrollRef,
  stickyHeaderIndices,
  refreshControl,
}: {
  children: ReactNode;
  scrollRef?: RefObject<ScrollView | null>;
  stickyHeaderIndices?: number[];
  refreshControl?: ReactElement<RefreshControlProps>;
}) {
  const cabecalho = useHeaderHeight();
  return (
    <Screen edges={IOS ? BORDAS_IOS : BORDAS_SEM_TOPO}>
      <ScrollView
        ref={scrollRef}
        stickyHeaderIndices={stickyHeaderIndices}
        refreshControl={refreshControl}
        style={IOS ? undefined : { marginTop: cabecalho }}
        contentInsetAdjustmentBehavior={IOS ? 'automatic' : undefined}
        contentContainerStyle={styles.conteudo}
        keyboardShouldPersistTaps="handled"
      >
        {children}
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  conteudo: {
    paddingTop: theme.base.space[3],
    paddingHorizontal: theme.base.space[3],
    paddingBottom: theme.base.space[8],
    gap: theme.base.space[4],
  },
}));
