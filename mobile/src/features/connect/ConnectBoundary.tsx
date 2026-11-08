import { Component, type ErrorInfo, type ReactNode } from 'react';
import { Pressable, Text } from 'react-native';
import { registrarDiag } from '@hangar/core';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

type Props = { children: ReactNode; resetKey: string };
type State = { failed: boolean; resetKey: string };

/**
 * Erro ao desenhar a área do Connect fica nela: sem isto, uma exceção de render no app de produção
 * fecha o app inteiro. O diário guarda o código para a próxima investigação.
 */
export class ConnectBoundary extends Component<Props, State> {
  state = { failed: false, resetKey: this.props.resetKey };

  static getDerivedStateFromError() { return { failed: true }; }

  // Outra conta é outra tela: a falha da anterior não fica de pé.
  static getDerivedStateFromProps(props: Props, state: State) {
    return props.resetKey === state.resetKey ? null : { failed: false, resetKey: props.resetKey };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    const message = String((error as Error)?.message ?? error).slice(0, 200);
    const where = (info.componentStack ?? '').trim().split('\n').slice(0, 6).map((line) => line.trim()).join(' < ');
    // O diário não pode derrubar o aviso que existe justamente para o app não cair.
    try { registrarDiag({ evento: 'connect.tela_falhou', nivel: 'erro', codigo: 'connect_screen_failed', detalhe: `${message} @ ${where}`.slice(0, 600) }); }
    catch { /* sem diário, o aviso continua na tela */ }
  }

  render() {
    if (!this.state.failed) return this.props.children;
    return <Failed onRetry={() => this.setState({ failed: false })} />;
  }
}

function Failed({ onRetry }: { onRetry: () => void }) {
  const c = useSettingsColors();
  return <Pressable accessibilityRole="button" accessibilityHint={m.connect_screen_retry_hint()} hitSlop={8} onPress={onRetry}
    style={{ minHeight: 44, justifyContent: 'center' }}>
    <Text style={{ color: c.text }}>{m.connect_screen_failed()}</Text>
  </Pressable>;
}
