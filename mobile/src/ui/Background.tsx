import { StyleSheet as RN, View } from 'react-native';
import { Image } from 'expo-image';
import { LinearGradient } from 'expo-linear-gradient';
import Svg, { Defs, Rect, RadialGradient, Stop } from 'react-native-svg';
import { useUnistyles } from 'react-native-unistyles';
import { useAparencia } from '../stores/aparencia';
import { toast } from './Toast';
import * as m from '../paraglide/messages';

// Mesmos modos do `lib/background.ts` da PWA, com os valores do app.css:
//   texture/aurora = faixa vertical (app.css:356 e :551);
//   aurora = a faixa MAIS uma luz fria no alto à esquerda (app.css:536-560), que não anima.
// Os hex são fixos de propósito, como lá: não saem da rampa de tokens.
const FAIXA = { dark: ['#0f0d12', '#141019'], light: ['#fbf8f4', '#f3eff2'] } as const;
// Alfa do centro da luz: 14% do acento no escuro, 9% no claro (o `color-mix` do app.css).
const LUZ = { dark: 0.14, light: 0.09 } as const;

export function Background() {
  const { theme, rt } = useUnistyles();
  const fundo = useAparencia((s) => s.fundo);
  const uri = useAparencia((s) => s.imagemUri);
  const claro = rt.themeName === 'light';
  const faixa = FAIXA[claro ? 'light' : 'dark'];
  return (
    // Camadas decorativas: o leitor de tela não tem o que anunciar aqui, e `pointerEvents` só
    // resolve o toque.
    <View
      style={RN.absoluteFill}
      pointerEvents="none"
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
    >
      {/* A cor opaca embaixo é sempre pintada: sem ela um modo translúcido deixaria a tela preta. */}
      <View style={[RN.absoluteFill, { backgroundColor: theme.tokens.bg.base }]} />
      {fundo === 'texture' || fundo === 'aurora' ? (
        <LinearGradient colors={faixa} style={RN.absoluteFill} />
      ) : null}
      {fundo === 'aurora' ? (
        // A caixa da luz é a do app.css: left -10%, top -25%, 70% × 85% da tela, elipse que
        // termina no lado mais próximo (= metade da caixa) e some em 72% do raio.
        <Svg style={styles.luz} pointerEvents="none">
          <Defs>
            <RadialGradient id="aurora" cx="50%" cy="50%" rx="50%" ry="50%">
              <Stop offset="0" stopColor={theme.tokens.accent.base} stopOpacity={LUZ[claro ? 'light' : 'dark']} />
              <Stop offset="0.72" stopColor={theme.tokens.accent.base} stopOpacity={0} />
            </RadialGradient>
          </Defs>
          <Rect x="0" y="0" width="100%" height="100%" fill="url(#aurora)" />
        </Svg>
      ) : null}
      {fundo === 'image' && uri ? (
        <Image
          source={{ uri }}
          style={RN.absoluteFill}
          contentFit="cover"
          transition={200}
          // Arquivo apagado por reinstalação ou limpeza: sem isto a preferência continua em `image`
          // e a tela fica chapada sem dizer por quê. Volta pro `flat`, que é o que está na tela.
          onError={(e) => {
            console.warn('Background: papel de parede não abriu', uri, e);
            toast.erro(m.erro_desconhecido());
            void useAparencia.getState().setImagemUri(null);
          }}
        />
      ) : null}
    </View>
  );
}

const styles = RN.create({
  luz: { position: 'absolute', left: '-10%', top: '-25%', width: '70%', height: '85%' },
});
