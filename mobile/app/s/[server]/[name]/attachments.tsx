import { Text, View } from 'react-native';
import { useLocalSearchParams } from 'expo-router';
import * as m from '../../../../src/paraglide/messages';

// Stub da Task 1: a Task 6 troca este corpo. A rota já existe pra o menu ⋯ navegar.
export default function AttachmentsSheet() {
  const { server, name } = useLocalSearchParams<{ server: string; name: string }>();
  return (
    <View style={{ flex: 1, padding: 16 }}>
      <Text>{m.ctx_anexos()}</Text>
      <Text>{String(server)} / {String(name)}</Text>
    </View>
  );
}
