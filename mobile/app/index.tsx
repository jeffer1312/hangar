import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Screen } from '../src/ui/Screen';
import { SessionList } from '../src/features/sessions/SessionList';
import * as m from '../src/paraglide/messages';

export default function Index() {
  const router = useRouter();
  return (
    <Screen>
      <View style={styles.header}>
        <Text style={styles.title}>{m.lista_titulo()}</Text>
        <Pressable
          onPress={() => router.push('/create' as never)}
          style={styles.fab}
          accessibilityLabel={m.sessao_nova?.() ?? 'Nova sessão'}
          accessibilityRole="button"
          hitSlop={8}
        >
          <Text style={styles.fabTxt}>＋</Text>
        </Pressable>
      </View>
      <View style={styles.listWrap}>
        <SessionList />
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: theme.base.space[4],
    paddingTop: theme.base.space[2],
    paddingBottom: theme.base.space[3],
  },
  title: {
    fontSize: 34,
    fontWeight: '700',
    color: theme.tokens.text.primary,
    letterSpacing: -0.5,
  },
  listWrap: { flex: 1 },
  fab: {
    width: 44,
    height: 44,
    borderRadius: 22,
    backgroundColor: theme.tokens.accent.base,
    justifyContent: 'center',
    alignItems: 'center',
  },
  fabTxt: { color: '#fff', fontSize: 22, fontWeight: '600', lineHeight: 22 },
}));
