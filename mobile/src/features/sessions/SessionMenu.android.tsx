import { View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { DropdownMenu, DropdownMenuItem, Host, Text } from '@expo/ui/jetpack-compose';
import type { Props } from './SessionMenu';
import * as m from '../../paraglide/messages';

// Menu nativo do Android (Compose). O `ContextMenu` do @expo/ui existe SÓ no swift-ui — o membro
// da família no jetpack-compose é o `DropdownMenu`, e ele é controlado por `expanded`, não por um
// comando imperativo: é justamente o que faz ele funcionar na New Architecture, onde o `show()`
// do @react-native-menu/menu é no-op.
// A âncora é um Host de 1px no rodapé da linha, sem toque: a linha em si continua sendo RN pura,
// fora do Compose, senão o arrasto do swipe passaria a atravessar a interop.
export function SessionMenu({ children, aberto, onFechar, temCwd, onRenomear, onGit, onLoop, onExcluir }: Props) {
  const { theme } = useUnistyles();
  // O slot de texto é uma view do Compose: string crua ali vira o erro "Text strings must be
  // rendered within a <Text>". Quem entra é o `Text` do próprio @expo/ui, não o do react-native.
  const item = (rotulo: string, acao: () => void, cor?: string) => (
    <DropdownMenuItem onClick={() => { onFechar(); acao(); }}>
      <DropdownMenuItem.Text>
        <Text color={cor}>{rotulo}</Text>
      </DropdownMenuItem.Text>
    </DropdownMenuItem>
  );

  return (
    <View>
      {children}
      <Host style={styles.ancora} pointerEvents="none">
        <DropdownMenu expanded={aberto} onDismissRequest={onFechar}>
          <DropdownMenu.Items>
            {item(m.sessao_renomear(), onRenomear)}
            {temCwd ? item('Git', onGit) : null}
            {item(m.loop_titulo(), onLoop)}
            {item(m.sessao_excluir_curto(), onExcluir, theme.tokens.status.error)}
          </DropdownMenu.Items>
        </DropdownMenu>
      </Host>
    </View>
  );
}

const styles = StyleSheet.create({
  ancora: { position: 'absolute', left: 24, bottom: 0, width: 1, height: 1 },
});
