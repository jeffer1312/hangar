import { useRouter } from 'expo-router';
import { Pagina } from '../../src/features/config/Pagina';
import { Linha } from '../../src/features/config/Linha';
import * as m from '../../src/paraglide/messages';

export default function ConfigIndex() {
  const router = useRouter();
  return (
    <Pagina>
      <Linha
        icon="Settings2"
        titulo={m.config_geral_titulo()}
        descricao={m.config_geral_descricao()}
        onPress={() => router.push('/config/geral' as never)}
      />
      <Linha
        icon="Palette"
        titulo={m.config_modal_aparencia()}
        descricao={m.config_modal_desc_aparencia()}
        onPress={() => router.push('/config/aparencia' as never)}
      />
      <Linha
        icon="Server"
        titulo={m.maquinas_titulo()}
        descricao={m.config_maquinas_desc()}
        onPress={() => router.push('/config/maquinas' as never)}
      />
      <Linha
        icon="Info"
        titulo={m.config_modal_sobre()}
        descricao={m.config_modal_desc_sobre()}
        onPress={() => router.push('/config/sobre' as never)}
      />
    </Pagina>
  );
}
