import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Screen } from '../src/ui/Screen';
import { Icon } from '../src/ui/Icon';
import { SessionList } from '../src/features/sessions/SessionList';
import { ServerSheet } from '../src/features/sessions/ServerSheet';
import * as m from '../src/paraglide/messages';

export default function Index() {
  const router = useRouter();
  const [servidoresAberto, setServidoresAberto] = useState(false);
  return (
    <Screen>
      <View style={styles.header}>
        <Text style={styles.title}>{m.lista_titulo()}</Text>
        <View style={styles.acoes}>
          <Pressable
            onPress={() => setServidoresAberto(true)}
            style={styles.icone}
            accessibilityLabel={m.maquinas_este_aparelho()}
            accessibilityRole="button"
            hitSlop={8}
          >
            <Icon name="Server" size={20} />
          </Pressable>
          <Pressable
            onPress={() => router.push('/config' as never)}
            style={styles.icone}
            accessibilityLabel={m.config_modal_titulo()}
            accessibilityRole="button"
            hitSlop={8}
          >
            <Icon name="Settings" size={20} />
          </Pressable>
          <Pressable
            onPress={() => router.push('/create' as never)}
            style={styles.fab}
            accessibilityLabel={m.sessao_nova()}
            accessibilityRole="button"
            hitSlop={8}
          >
            <Text style={styles.fabTxt}>＋</Text>
          </Pressable>
        </View>
      </View>
      <View style={styles.listWrap}>
        <SessionList />
      </View>
      <ServerSheet open={servidoresAberto} onFechar={() => setServidoresAberto(false)} />
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
  acoes: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  icone: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
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
