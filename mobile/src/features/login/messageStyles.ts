import { StyleSheet } from 'react-native-unistyles';

/** Erro e espera das telas de entrada, iguais no formulário do Connect e no de endereço. */
export const messageStyles = StyleSheet.create((theme) => ({
  errorBox: {
    backgroundColor: theme.tokens.status.error + '14',
    borderWidth: 1,
    borderColor: theme.tokens.status.error + '33',
    borderRadius: theme.base.radius.sm,
    padding: theme.base.space[3],
  },
  errorText: { fontSize: theme.base.text.sm, color: theme.tokens.status.error },
  waitText: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center' },
}));
