import { useEffect, useRef, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { MultiTextInput } from '../../vendor/happy/components/MultiTextInput';
import * as m from '../../paraglide/messages';

interface Props {
  path: string;
  initialText: string;
  onSalvar: (texto: string) => Promise<string | null>;
  onDescartar: () => void;
}

export function FileEditor({ path, initialText, onSalvar, onDescartar }: Props) {
  const { theme } = useUnistyles();
  const [texto, setTexto] = useState(initialText);
  const [salvando, setSalvando] = useState(false);
  const [erro, setErro] = useState<string | null>(null);
  const [salvo, setSalvo] = useState(false);
  const salvoTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => setTexto(initialText), [initialText]);

  useEffect(() => {
    return () => {
      if (salvoTimer.current) clearTimeout(salvoTimer.current);
    };
  }, []);

  const sujo = texto !== initialText;

  async function handleSalvar() {
    if (!sujo || salvando) return;
    setSalvando(true);
    setErro(null);
    const falha = await onSalvar(texto);
    setSalvando(false);
    if (falha) {
      setErro(falha);
      return;
    }
    setSalvo(true);
    if (salvoTimer.current) clearTimeout(salvoTimer.current);
    salvoTimer.current = setTimeout(() => setSalvo(false), 2000);
  }

  function handleDescartar() {
    if (sujo) {
      // avisa que há mudança não salva — o caller decide se fecha
    }
    onDescartar();
  }

  return (
    <View style={styles.root}>
      <View style={[styles.bar, { borderBottomColor: theme.tokens.border.subtle }]}>
        <Text style={[styles.path, { color: theme.tokens.text.primary }]} numberOfLines={1}>
          {path}
        </Text>
        {sujo ? <Text style={[styles.naoSalvo, { color: theme.tokens.status.warning }]}>• {m.arq_nao_salvo()}</Text> : null}
        <Pressable onPress={handleDescartar} style={styles.btn} accessibilityRole="button">
          <Text style={[styles.btnTxt, { color: theme.tokens.text.secondary }]}>{m.arq_descartar()}</Text>
        </Pressable>
        <Pressable
          onPress={handleSalvar}
          disabled={!sujo || salvando}
          style={[styles.btnPrim, !sujo || salvando ? { opacity: 0.5 } : null]}
          accessibilityRole="button"
        >
          <Text style={[styles.btnPrimTxt, { color: '#fff' }]}>{salvando ? m.arq_salvando() : m.arq_salvar()}</Text>
        </Pressable>
      </View>
      {salvo ? <Text style={[styles.salvo, { color: theme.tokens.status.success }]}>✓ {m.arq_salvo()}</Text> : null}
      {erro ? (
        <Text style={[styles.erro, { color: theme.tokens.status.error }]} accessibilityRole="alert">
          {erro}
        </Text>
      ) : null}
      <View style={styles.editor}>
        <MultiTextInput value={texto} onChangeText={setTexto} placeholder="" />
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: {
    flex: 1,
    backgroundColor: theme.tokens.bg.base,
  },
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    borderBottomWidth: 1,
    gap: theme.base.space[2],
  },
  path: {
    flex: 1,
    fontSize: 12,
    fontWeight: '500',
  },
  naoSalvo: {
    fontSize: 11,
  },
  btn: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
  },
  btnTxt: {
    fontSize: 12,
    fontWeight: '500',
  },
  btnPrim: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 8,
    backgroundColor: theme.tokens.accent.base,
  },
  btnPrimTxt: {
    fontSize: 12,
    fontWeight: '600',
  },
  salvo: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: 4,
  },
  erro: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: 4,
  },
  editor: {
    flex: 1,
    padding: theme.base.space[2],
  },
}));
