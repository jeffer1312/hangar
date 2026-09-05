import type { ReactNode } from 'react';
import { Platform, ScrollView } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Screen } from '../../ui/Screen';

// O cabeçalho das telas de config é transparente (o papel de parede passa por baixo), então o
// conteúdo começa embaixo dele. ponytail: constante em vez de `useHeaderHeight` — o expo-router
// não reexporta o hook do react-navigation; se o header ganhar altura própria, o número vem junto.
const CABECALHO = Platform.OS === 'ios' ? 44 : 56;

/** Casca das telas de `/config`: papel de parede, área segura e rolagem com o recuo do cabeçalho. */
export function Pagina({ children }: { children: ReactNode }) {
  return (
    <Screen>
      <ScrollView contentContainerStyle={styles.conteudo} keyboardShouldPersistTaps="handled">
        {children}
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  conteudo: {
    paddingTop: CABECALHO + theme.base.space[3],
    paddingHorizontal: theme.base.space[3],
    paddingBottom: theme.base.space[8],
    gap: theme.base.space[2],
  },
}));
