import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Screen } from '../src/ui/Screen';
import { SessionList } from '../src/features/sessions/SessionList';
import * as m from '../src/paraglide/messages';

export default function Index() {
  return (
    <Screen>
      <View style={styles.header}>
        <Text style={styles.title}>{m.lista_titulo()}</Text>
      </View>
      <View style={styles.listWrap}>
        <SessionList />
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  header: {
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
}));
