import { Platform, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter, useSegments } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { Screen } from '../src/ui/Screen';
import { CreateSessionSheet } from '../src/features/create/CreateSessionSheet';

// Nova sessão do menu: o formulário completo, com Cancelar e Criar no próprio cabeçalho.
export default function CreateRoute() {
  const router = useRouter();
  const segments = useSegments();
  // `?server=&engine=`: o assistente do CLIProxyAPI abre a conversa já no motor que acabou de criar.
  const { server, engine } = useLocalSearchParams<{ server?: string; engine?: string }>();
  const close = () => {
    if (router.canGoBack()) router.back();
    else router.replace('/');
  };
  return (
    <Screen independent={Platform.OS !== 'android'}>
      <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.wrap}>
        {segments[0] === 'create' ? <CreateSessionSheet layout="form" onClose={close} initial={{ server, engine }} /> : <View />}
      </KeyboardAvoidingView>
    </Screen>
  );
}

// Transparente: o Background da Screen pinta o fundo escolhido em Aparência.
const styles = StyleSheet.create(() => ({
  wrap: { flex: 1, backgroundColor: 'transparent' },
}));
