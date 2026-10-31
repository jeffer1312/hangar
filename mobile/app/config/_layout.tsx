import { Stack, useRouter } from 'expo-router';
import { HeaderBack, useGlassHeader } from '../../src/ui/glassHeader';
import * as m from '../../src/paraglide/messages';

// Voz mantém o título na barra durante a rolagem; o voltar leva o nome da página anterior.
export default function ConfigLayout() {
  const router = useRouter();
  return (
    <Stack screenOptions={{ ...useGlassHeader(), headerBackButtonDisplayMode: 'default' }}>
      <Stack.Screen
        name="index"
        options={{
          title: m.config_modal_titulo(),
          // Raiz da pilha das configurações não ganha "voltar" sozinha: sai para a tela de onde veio.
          headerLeft: () => <HeaderBack onPress={() => router.back()} />,
        }}
      />
      <Stack.Screen name="voice" options={{ title: m.native_settings_page_voice() }} />
      <Stack.Screen name="cofre" options={{ title: m.vault_app_title() }} />
    </Stack>
  );
}
