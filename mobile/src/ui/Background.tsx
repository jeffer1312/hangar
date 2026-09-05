import { StyleSheet as RN, View } from 'react-native';
import { Image } from 'expo-image';
import { LinearGradient } from 'expo-linear-gradient';
import { useUnistyles } from 'react-native-unistyles';
import { useAparencia } from '../stores/aparencia';

// Atrás de tudo. flat = cor do tema; texture = gradiente sutil; aurora = dois gradientes cruzados
// do acento; image = foto do aparelho (copiada pro diretório do app no setImagemUri).
// A camada opaca de baixo é sempre pintada: sem ela um modo translúcido deixaria a tela preta.
export function Background() {
  const { theme } = useUnistyles();
  const fundo = useAparencia((s) => s.fundo);
  const uri = useAparencia((s) => s.imagemUri);
  const base = theme.tokens.bg.base;
  const a = theme.tokens.accent.base;
  return (
    <View style={RN.absoluteFill} pointerEvents="none">
      <View style={[RN.absoluteFill, { backgroundColor: base }]} />
      {fundo === 'texture' ? (
        <LinearGradient colors={[base, theme.tokens.bg.surface, base]} style={RN.absoluteFill} />
      ) : null}
      {fundo === 'aurora' ? (
        <>
          <LinearGradient colors={[a + '66', 'transparent']} start={{ x: 0, y: 0 }} end={{ x: 1, y: 1 }} style={RN.absoluteFill} />
          <LinearGradient colors={['transparent', theme.tokens.chart[2] + '44']} start={{ x: 1, y: 0 }} end={{ x: 0, y: 1 }} style={RN.absoluteFill} />
        </>
      ) : null}
      {fundo === 'image' && uri ? (
        <Image source={{ uri }} style={RN.absoluteFill} contentFit="cover" transition={200} />
      ) : null}
    </View>
  );
}
