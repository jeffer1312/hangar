import { Component, type ReactNode } from 'react';
import { Pressable, Text } from 'react-native';
import { registrarDiag } from '@hangar/core';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

/**
 * Erro ao desenhar a área do Connect fica nela: sem isto, uma exceção de render no app de produção
 * fecha o app inteiro. O diário guarda o código para a próxima investigação.
 */
export class ConnectBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() { return { failed: true }; }

  componentDidCatch(error: unknown) {
    registrarDiag({ evento: 'connect.tela_falhou', nivel: 'erro', codigo: 'connect_screen_failed', detalhe: String((error as Error)?.message ?? error).slice(0, 200) });
  }

  render() {
    if (!this.state.failed) return this.props.children;
    return <Failed onRetry={() => this.setState({ failed: false })} />;
  }
}

function Failed({ onRetry }: { onRetry: () => void }) {
  const c = useSettingsColors();
  return <Pressable accessibilityRole="button" onPress={onRetry} style={{ minHeight: 44, justifyContent: 'center' }}>
    <Text style={{ color: c.text }}>{m.connect_screen_failed()}</Text>
  </Pressable>;
}
