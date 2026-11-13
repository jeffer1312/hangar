import { Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { exitCode } from './desktopLook';
import * as m from '../../paraglide/messages';

/** "Código de saída N" acima da saída aberta, em todo estilo; sem `Exit code N` na saída, nada. */
export function ExitCodeLine({ output }: { output: string | null | undefined }) {
  const { theme } = useUnistyles();
  const code = exitCode(output);
  if (code === null) return null;
  return <Text style={[styles.exit, { color: theme.tokens.status.warning }]}>{m.ferr_desktop_codigo_saida({ n: code })}</Text>;
}

const styles = StyleSheet.create((theme) => ({
  exit: { fontSize: theme.base.text.xs },
}));
