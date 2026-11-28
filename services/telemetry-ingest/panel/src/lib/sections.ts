import { Cpu, LayoutGrid, MessagesSquare, Monitor, Route, ScrollText, Settings2, Smartphone, Table2, Timer, TriangleAlert, Zap, type LucideIcon } from "lucide-react";

// Cada seção tem cor e ícone próprios: o item da navegação, o título e as barras dela usam a mesma cor.
export const SECTIONS: { id: string; name: string; hue: string; icon: LucideIcon; explain: string }[] = [
  { id: "overview", name: "Visão geral", hue: "#8b93f0", icon: LayoutGrid, explain: "Quantas máquinas e pessoas usam o Hangar, o que mais falhou e quem aceitou mandar dados." },
  { id: "problems", name: "Problemas", hue: "#f2766b", icon: TriangleAlert, explain: "O que deu errado para quem usa: ação que falhou, sessão que caiu, app que fechou." },
  { id: "log", name: "Log de erros", hue: "#f2a36b", icon: ScrollText, explain: "Linhas de aviso e erro do servidor, limpas na máquina antes de sair." },
  { id: "agents", name: "Uso por agente", hue: "#45c2a8", icon: MessagesSquare, explain: "Mensagens enviadas a cada agente, dia a dia." },
  { id: "top", name: "O que mais usam", hue: "#66c7e8", icon: Zap, explain: "Os recursos do app e as ferramentas do Hangar que os agentes mais chamam." },
  { id: "effort", name: "Modelo e esforço", hue: "#a8aff3", icon: Cpu, explain: "Com que modelo e esforço as sessões abrem." },
  { id: "places", name: "De onde usam", hue: "#e8b45a", icon: Smartphone, explain: "Em que app as pessoas ficam e se as sessões têm terminal." },
  { id: "speed", name: "Velocidade", hue: "#5a9fe8", icon: Timer, explain: "Quanto a pessoa espera em cada momento do Hangar. \"Normalmente\" é o tempo de metade das vezes; \"9 em 10\" mostra o tempo das vezes lentas." },
  { id: "trail", name: "Caminho de uso", hue: "#b5d65a", icon: Route, explain: "Por onde as pessoas passam no app, passo a passo, só pelo nome de cada ação." },
  { id: "machines", name: "Máquinas", hue: "#b48cf0", icon: Monitor, explain: "Versão, sistema e canal de cada máquina, e a saúde do servidor." },
  { id: "settings", name: "Como configuram", hue: "#e88ab8", icon: Settings2, explain: "Quantas máquinas escolheram cada opção. Cada app manda o retrato uma vez por dia." },
  { id: "raw", name: "Dados brutos", hue: "#a9a0a4", icon: Table2, explain: "Cada contador como chegou, para conferir um número." },
];
export const section = (id: string) => SECTIONS.find(s => s.id === id)!;
