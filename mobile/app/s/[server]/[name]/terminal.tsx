import { View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useLocalSearchParams } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { TerminalMirror } from '../../../../src/features/terminal/TerminalMirror';

export default function TerminalSheet() {
  const { server, name, aviso } = useLocalSearchParams<{ server: string; name: string; aviso?: string }>();
  const sessionName = Array.isArray(name) ? name[0] : (name ?? '');
  const avisoText = Array.isArray(aviso) ? aviso[0] : aviso;
  return (
    <KeyboardAvoidingView behavior="padding" style={styles.root}>
      <View style={styles.inner}>
        <TerminalMirror name={sessionName} aviso={avisoText} />
      </View>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: {
    flex: 1,
    backgroundColor: theme.tokens.bg.base,
  },
  inner: {
    flex: 1,
  },
}));
