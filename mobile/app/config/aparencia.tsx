import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as ImagePicker from 'expo-image-picker';
import { themeDark, themeLight } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { Linha } from '../../src/features/config/Linha';
import { Segmentado, type Opcao } from '../../src/features/config/Segmentado';
import { Slider } from '../../src/features/config/Slider';
import { Icon } from '../../src/ui/Icon';
import { toast } from '../../src/ui/Toast';
import { useAparencia, type Fundo } from '../../src/stores/aparencia';
import * as m from '../../src/paraglide/messages';

// As mesmas cores que a PWA oferece (SW_DESTAQUE de components/settings/CorTemaSettings.svelte).
const PALETA = ['#d97757', '#34c759', '#0ea5e9', '#e879f9', '#f59e0b'] as const;

export default function Aparencia() {
  const { theme, rt } = useUnistyles();
  const fundo = useAparencia((s) => s.fundo);
  const imagemUri = useAparencia((s) => s.imagemUri);
  const panelAlpha = useAparencia((s) => s.panelAlpha);
  const surfaceAlpha = useAparencia((s) => s.surfaceAlpha);
  const acento = useAparencia((s) => s.acento);
  const acentoFabrica = rt.themeName === 'dark' ? themeDark.accent.base : themeLight.accent.base;

  const FUNDOS: ReadonlyArray<Opcao<Fundo>> = [
    { v: 'flat', label: m.config_fundo_liso(), aria: m.config_fundo_chapado() },
    { v: 'texture', label: m.config_fundo_textura(), aria: m.config_fundo_grao() },
    { v: 'aurora', label: m.config_fundo_luz(), aria: m.config_fundo_aurora() },
    { v: 'image', label: m.config_fundo_imagem(), aria: m.config_fundo_usar_imagem() },
  ];

  // Três desfechos diferentes, três avisos diferentes: sem a permissão a galeria nem abre e o
  // resultado é indistinguível de um cancelamento — o toque parecia não fazer nada.
  const escolherImagem = async () => {
    try {
      const permissao = await ImagePicker.requestMediaLibraryPermissionsAsync();
      if (!permissao.granted) {
        toast.erro(m.aparencia_fundo_sem_permissao());
        return;
      }
      const r = await ImagePicker.launchImageLibraryAsync({ mediaTypes: ['images'], quality: 0.8 });
      if (r.canceled) return;
      if (!r.assets[0]) {
        console.warn('Aparência: picker voltou sem cancelar e sem imagem');
        toast.erro(m.aparencia_fundo_erro_galeria());
        return;
      }
      // A falha da CÓPIA é avisada pelo próprio store, com a mensagem dela.
      await useAparencia.getState().setImagemUri(r.assets[0].uri);
    } catch {
      toast.erro(m.aparencia_fundo_erro_galeria());
    }
  };

  return (
    <Pagina>
      <Linha titulo={m.config_fundo_curto()}>
        <Segmentado opcoes={FUNDOS} valor={fundo} onChange={(v) => useAparencia.getState().setFundo(v)} rotulo={m.config_fundo_curto()} />
      </Linha>

      {fundo === 'image' ? (
        <Linha titulo={m.config_fundo_escolher()} onPress={() => void escolherImagem()}>
          {imagemUri ? (
            <Pressable
              onPress={() => void useAparencia.getState().setImagemUri(null)}
              accessibilityRole="button"
              accessibilityLabel={m.config_fundo_remover()}
              style={styles.remover}
            >
              <Icon name="Trash2" size={16} color={theme.tokens.status.error} />
              <Text style={[styles.removerTxt, { color: theme.tokens.status.error }]}>
                {m.config_fundo_remover()}
              </Text>
            </Pressable>
          ) : null}
        </Linha>
      ) : null}

      <Linha titulo={m.config_fundo_transparencia()} descricao={m.config_fundo_transparencia_detalhe()}>
        <Slider
          valor={panelAlpha}
          min={0.3}
          max={1}
          onChange={(v) => useAparencia.getState().setPanelAlpha(v)}
          label={m.config_fundo_transparencia()}
        />
      </Linha>

      <Linha titulo={m.config_fundo_solidez()} descricao={m.config_fundo_solidez_detalhe()}>
        <Slider
          valor={surfaceAlpha}
          min={0}
          max={1}
          onChange={(v) => useAparencia.getState().setSurfaceAlpha(v)}
          label={m.config_fundo_solidez()}
        />
      </Linha>

      <Linha titulo={m.config_aparencia_cor_tema()} descricao={m.config_aparencia_cor_tema_desc()}>
        <View style={styles.paleta}>
          <Circulo
            cor={acentoFabrica}
            selecionado={acento === null}
            rotulo={m.config_aparencia_voltar_padrao()}
            onPress={() => useAparencia.getState().setAcento(null)}
          />
          {PALETA.map((c) => (
            <Circulo
              key={c}
              cor={c}
              selecionado={acento === c}
              rotulo={c}
              onPress={() => useAparencia.getState().setAcento(c)}
            />
          ))}
        </View>
      </Linha>
    </Pagina>
  );
}

function Circulo({
  cor,
  selecionado,
  rotulo,
  onPress,
}: {
  cor: string;
  selecionado: boolean;
  rotulo: string;
  onPress: () => void;
}) {
  const { theme } = useUnistyles();
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={rotulo}
      accessibilityState={{ selected: selecionado }}
      hitSlop={6}
      style={[
        styles.circulo,
        { backgroundColor: cor, borderColor: selecionado ? theme.tokens.text.primary : 'transparent' },
      ]}
    />
  );
}

const styles = StyleSheet.create((theme) => ({
  paleta: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], flexWrap: 'wrap' },
  circulo: { width: 32, height: 32, borderRadius: 16, borderWidth: 2 },
  remover: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], alignSelf: 'flex-start' },
  removerTxt: { fontSize: theme.base.text.sm, fontWeight: '600' },
}));
