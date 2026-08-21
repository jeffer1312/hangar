import { Text } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Screen } from '../src/ui/Screen';
import { Glass } from '../src/ui/Glass';

export default function Index() {
  return (
    <Screen>
      <Glass style={styles.card}>
        <Text style={styles.t}>Hangar</Text>
      </Glass>
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  card: { margin: theme.base.space[4], padding: theme.base.space[4] },
  t: { color: theme.tokens.text.primary, fontSize: theme.base.text.lg },
}));
