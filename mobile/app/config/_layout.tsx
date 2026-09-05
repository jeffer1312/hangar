import { Stack } from 'expo-router';
import { useUnistyles } from 'react-native-unistyles';
import * as m from '../../src/paraglide/messages';

// Cabeçalho transparente pra o papel de parede atravessar; o recuo do conteúdo vem da `Pagina`.
export default function ConfigLayout() {
  const { theme } = useUnistyles();
  return (
    <Stack
      screenOptions={{
        headerShown: true,
        headerTransparent: true,
        headerBlurEffect: 'regular',
        headerTintColor: theme.tokens.accent.base,
        headerTitleStyle: { color: theme.tokens.text.primary },
      }}
    >
      <Stack.Screen name="index" options={{ title: m.config_modal_titulo() }} />
      <Stack.Screen name="geral" options={{ title: m.config_geral_titulo() }} />
      <Stack.Screen name="aparencia" options={{ title: m.config_modal_aparencia() }} />
      <Stack.Screen name="maquinas" options={{ title: m.maquinas_titulo() }} />
      <Stack.Screen name="sobre" options={{ title: m.config_modal_sobre() }} />
    </Stack>
  );
}
