import { DevSettings } from 'react-native';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader } from '../../src/features/config/PageHeader';
import { SectionCard } from '../../src/features/config/SectionCard';
import { SettingsRow } from '../../src/features/config/SettingsRow';
import { DeliveryModeRow } from '../../src/features/config/DeliveryModeRow';
import { Segmented, type Option } from '../../src/features/config/Segmented';
import { useAparencia, type Idioma } from '../../src/stores/aparencia';
import { toast } from '../../src/ui/Toast';
import type { GroupBy } from '@hangar/core';
import * as m from '../../src/paraglide/messages';

// Tema e "O que entra no pensamento" moram na Aparência, como no desktop Rust.
export default function Geral() {
  const idioma = useAparencia((s) => s.idioma);
  const agrupar = useAparencia((s) => s.agrupar);

  const IDIOMAS: ReadonlyArray<Option<Idioma>> = [
    { v: 'system', label: m.config_tema_auto(), aria: m.config_idioma_sistema() },
    { v: 'pt', label: m.config_idioma_pt() },
    { v: 'en', label: m.config_idioma_en() },
  ];
  const AGRUPAR: ReadonlyArray<Option<GroupBy>> = [
    { v: 'server', label: m.lista_agrupar_servidor() },
    { v: 'project', label: m.lista_agrupar_projeto() },
    { v: 'none', label: m.lista_agrupar_nenhum() },
  ];

  // Trocar o idioma não troca as mensagens já carregadas (o paraglide as compila em funções): em
  // dev o recarregador resolve na hora, em produção só na próxima abertura — e o aviso diz qual.
  const trocarIdioma = (v: Idioma) => {
    useAparencia.getState().setIdioma(v);
    toast.ok(__DEV__ ? m.config_idioma_nota_reload() : m.config_idioma_nota_proxima());
    if (__DEV__) DevSettings.reload();
  };

  return (
    <Pagina>
      <PageHeader title={m.config_geral_titulo()} subtitle={m.config_geral_lead()} />
      <SectionCard>
        <SettingsRow icon="Languages" title={m.config_idioma_rotulo()} description={m.config_idioma_nota_proxima()}>
          <Segmented options={IDIOMAS} value={idioma} onChange={trocarIdioma} label={m.config_idioma_rotulo()} />
        </SettingsRow>
        <SettingsRow icon="ListTree" title={m.lista_agrupar()}>
          <Segmented options={AGRUPAR} value={agrupar} onChange={(v) => useAparencia.getState().setAgrupar(v)} label={m.lista_agrupar()} />
        </SettingsRow>
      </SectionCard>
      <SectionCard icon="MessageSquare" title={m.native_settings_conversation_group()}>
        <DeliveryModeRow />
      </SectionCard>
    </Pagina>
  );
}
