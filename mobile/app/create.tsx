import { View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Screen } from '../src/ui/Screen';
import { CreateSessionSheet } from '../src/features/create/CreateSessionSheet';

export default function CreateRoute() {
  const router = useRouter();
  return (
    <Screen>
      <View style={styles.wrap}>
        <CreateSessionSheet onClose={() => router.back()} />
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: { flex: 1, backgroundColor: theme.tokens.bg.base },
}));
