import { useEffect, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Sheet } from '../../ui/Sheet';
import { MultilineInput } from '../../ui/MultilineInput';
import * as m from '../../paraglide/messages';
import { superficie } from '../../theme/superficie';

// Renomear pede um campo de texto, e `Alert.prompt` só existe no iOS — uma folha serve os dois
// sistemas com o mesmo código.
export function RenameSheet({
  nome,
  onConfirmar,
  onFechar,
}: {
  nome: string | null;
  onConfirmar: (novo: string) => void;
  onFechar: () => void;
}) {
  const { theme } = useUnistyles();
  const [valor, setValor] = useState('');
  useEffect(() => { if (nome) setValor(nome); }, [nome]);
  const limpo = valor.trim();

  return (
    <Sheet open={!!nome} sizes={['auto']} onDismiss={onFechar}>
      <View style={styles.inner}>
        <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.sessao_renomear()}</Text>
        <View style={[styles.campo, { backgroundColor: superficie(theme, 0.8), borderColor: theme.tokens.border.subtle }]}>
          <MultilineInput
            multiline={false}
            value={valor}
            onChangeText={setValor}
            placeholder={m.sessao_novo_nome()}
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="done"
            onSubmitEditing={() => { if (limpo && limpo !== nome) onConfirmar(limpo); }}
          />
        </View>
        <View style={styles.botoes}>
          <Pressable onPress={onFechar} style={styles.botao} accessibilityRole="button">
            <Text style={{ color: theme.tokens.text.secondary }}>{m.comum_cancelar()}</Text>
          </Pressable>
          <Pressable
            onPress={() => onConfirmar(limpo)}
            disabled={!limpo || limpo === nome}
            style={styles.botao}
            accessibilityRole="button"
          >
            <Text style={{ color: limpo && limpo !== nome ? theme.tokens.accent.base : theme.tokens.text.muted, fontWeight: '600' }}>
              {m.comum_confirmar()}
            </Text>
          </Pressable>
        </View>
      </View>
    </Sheet>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: { padding: theme.base.space[4], gap: theme.base.space[3] },
  title: { fontSize: theme.base.text.lg, fontWeight: '600' },
  campo: { borderRadius: theme.base.radius.md, borderWidth: 1 },
  botoes: { flexDirection: 'row', justifyContent: 'flex-end', gap: theme.base.space[2] },
  botao: { minHeight: 44, paddingHorizontal: theme.base.space[3], justifyContent: 'center' },
}));
