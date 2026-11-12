# Hangar — app nativo (Expo)

App Android/iPhone que conversa com um servidor Hangar que já está rodando (o backend de
`backend/`). Não sobe servidor próprio. Sem nenhum servidor pareado, o app abre na tela de login;
com um servidor pareado fora do ar, abre na lista de sessões e o servidor continua salvo. Ele só sai
se o token for recusado ou se você removê-lo; sem nenhum servidor restante, o app volta ao login.

## Conectar ao servidor

1. O backend imprime no boot um QR com URL + token (o mesmo do PWA, ver `docs/USAGE.md`).
2. No app: **Escanear QR**, ou digitar a URL (`http://<ip-da-lan>:8765` ou o endereço Tailscale) e o token.
3. O token fica no SecureStore do aparelho. Token recusado mostra a causa na tela de login;
   re-pareie pelo QR se ele foi trocado no servidor.

Rede: o app aceita `http` em LAN/VPN (`usesCleartextTraffic` no Android,
`NSAllowsArbitraryLoads` + permissão de rede local no iOS).

## Dependências

O app é um workspace do monorepo e importa `@hangar/core` por `file:../packages/core`.

```bash
(cd mobile && npm install)        # lockfile próprio de mobile/
```

- Não tire `mobile` dos `workspaces` da raiz: o EAS detecta o monorepo por eles e envia o
  `packages/core` junto. Detalhe em `docs/decisoes/instalacao.md` (instalação seletiva).
- O `npm ci` do CI e dos instaladores é seletivo (`@hangar/core` + `frontend`) e **não** instala
  o app.
- `react-dom` (só para testes de componente) acompanha a versão exata de `react`.

## Traduções

As pastas `src/paraglide` (app) e `packages/core/src/paraglide` (core) são geradas e ignoradas
pelo git. Os scripts `start`, `android`, `ios`, `test` e `typecheck` compilam a do app. No EAS,
o hook `eas-build-post-install` compila as duas antes do bundle; sem a do core o build quebra em
`Unable to resolve module .../paraglide/messages`.

## Perfis de build (`eas.json`)

| Perfil | Para quê | Situação |
|---|---|---|
| `preview` | uso diário: APK Android e iOS ad hoc, instalação interna, sem Metro | caminho de instalação deste README |
| `development` | `developmentClient: true`, carrega o JS do Metro | iPhone sem conta paga: ver "iPhone com Apple ID gratuito" |
| `production` | loja (Android: teste fechado), canal OTA `preview` | `eas submit --platform android --id <build>` envia à faixa `alpha` com a chave em `~/.config/hangar/play-service-account.json` (conta de serviço `eas-submit` do projeto Google Cloud `hangar-play`) |

## Atualizações OTA (EAS Update)

O Release do GitHub e os builds EAS `preview` e `production` recebem o canal `preview`.
Instale primeiro um binário novo com `expo-updates`; o app já instalado sem ele não recebe OTA.
Publique a partir de uma árvore com uma instalação só, a mesma do workflow Mobile OTA:
`rm -rf mobile/node_modules` e, na raiz, `npm ci --workspace=@hangar/core --workspace=mobile`.
A instalação de desenvolvimento (`npm install` em `mobile/`, que os testes usam) somada à da raiz
põe duas cópias do React no bundle, e a OTA quebra ao abrir. O aparelho marca essa atualização
como falha, fica na anterior sem aviso e não a tenta de novo; o `metro.config.js` recusa esse
arranjo (`scripts/single-react.cjs`).
Compile as traduções em `mobile/`: `npm run i18n:compile:core && npm run i18n:compile`.
Publique em `mobile/`: `eas update --channel preview --message "Descrição da correção" --environment preview` (troque ambos para `production` no outro canal).
Uma OTA que o aparelho baixa a cada abertura (`Update available` no logcat de `dev.expo.updates`) e
nunca aplica falhou ao abrir: publique de novo, depois de corrigir, que ela ganha outro ID.
Ao abrir, o app baixa em segundo plano, sem segurar a tela; a correção entra no próximo início completo (feche e abra de novo).
O runtime é uma versão nativa explícita em `expo.runtimeVersion` (`native-3`, no formato `native-N`), independente de `expo.version` e do `VERSION` da raiz (ambos continuam em `0.1.0`).
Mudança de plugin, SDK, dependência ou configuração nativa exige incrementar `native-N` e gerar/instalar um binário novo antes de publicar OTA para esse runtime.
O binário já instalado com runtime anterior (`0.1.0`, `native-2`) não recebe OTA de `native-3`; primeiro instale o novo binário por cima, preservando os dados.
Metro, Expo Go e development client mantêm OTA automática desligada; no GitHub, selecione `Release` para OTA.
Veja publicações, canal e runtime em [Expo → Hangar → Updates](https://expo.dev/accounts/jeffer1312/projects/hangar/updates) ou `eas update:list`.
OTA não renova o perfil Apple gratuito de 7 dias: a assinatura ainda precisa ser renovada.

## Versão

- Versão visível (`app.json` → `expo.version`, mostrada na tela Sobre das configurações) = `major.minor.patch`
  do arquivo `VERSION` da raiz. Muda à mão, junto com o `VERSION`.
- Número do build (Android `versionCode`, iOS `buildNumber`) mora no servidor EAS
  (`appVersionSource: remote`) e sobe sozinho a cada build `preview` (`autoIncrement`). Android só
  instala por cima um APK com o mesmo pacote (`com.hangar.mobile`), a mesma assinatura e número
  igual ou maior.
- A tela Sobre não mostra o número do build: para saber qual binário está instalado, compare
  `appBuildVersion`/`gitCommitHash` do `eas build:list` com
  `adb shell dumpsys package com.hangar.mobile | grep version`.
- Não troque o `package`/`bundleIdentifier` para instalar "ao lado": vira outro app, sem os dados.

```bash
(cd mobile && eas build:version:get --platform android --profile preview)
```

## Gerar os binários

CLI: `eas-cli` 23.2.0 (o `eas.json` exige `>= 12.0.0`). Rode sempre em subshell dentro de
`mobile/`, nunca na raiz: na raiz o CLI não acha o projeto e pode deixar um `app.json` vazio.
O EAS empacota a árvore de trabalho, incluindo alterações em arquivos rastreados ainda
não commitadas. Gere da revisão conferida e registre também o diff: o `gitCommitHash` do
build identifica o HEAD e, sozinho, não identifica alterações locais enviadas.

```bash
(cd mobile && eas build --platform android --profile preview)
(cd mobile && eas build --platform ios --profile preview)
```

- **Android**: o keystore mora no servidor Expo e as builds `preview` reutilizam o mesmo, o que
  mantém a atualização por cima. Não há keystore no repositório; não gere outro.
- **iOS**: precisa de conta Apple Developer ligada ao projeto EAS, certificado de distribuição e
  perfil ad hoc com o UDID de cada iPhone (`eas device:create`). Criar essas credenciais exige
  login Apple interativo: `(cd mobile && eas credentials --platform ios)`, ou o `eas build` acima
  sem `--non-interactive`. Aparelho fora do perfil não instala.

`npx expo export` só prova o bundle JS; não substitui a compilação nativa nem conta como APK.

## iPhone com Apple ID gratuito (sem Mac)

Dev build: instala uma vez e depois cada edição aparece na hora, pelo Metro.

1. GitHub → Actions → **iOS dev build** → *Run workflow*. Baixe o artefato `hangar-dev-ipa`
   (`.ipa` sem assinatura). Rodar de novo só quando mudar dependência nativa ou `app.json`.
2. Instale com o Apple ID gratuito pelo [SideStore](https://sidestore.io) (reassina no próprio
   iPhone pelo Wi-Fi; o pareamento inicial sai de `jitterbugpair`, com o iPhone no USB).
   No iPhone, ligue Ajustes → Privacidade e Segurança → Modo de Desenvolvedor.
3. Na máquina: `(cd mobile && npm start)`. Pelo Tailscale, defina antes
   `REACT_NATIVE_PACKAGER_HOSTNAME=<ip-tailscale-da-máquina>`. No app, abra a URL que o Metro
   mostra.

Limites do Apple ID gratuito: a assinatura vale 7 dias (o SideStore renova), no máximo 3 apps
instalados assim e sem push da Apple.

## Instalar e atualizar mantendo os dados

- Android: baixe o APK pelo link do build e instale; por cabo,
  `adb install -r <arquivo>.apk`.
- iOS: abra o link interno do build no iPhone registrado.
- Atualizar = instalar o binário novo **por cima** do anterior. Não desinstale nem limpe os dados:
  servidores/token (SecureStore), projeto por máquina, rascunhos, tema, acento e papel de parede
  ficam no armazenamento do app e só sobrevivem à atualização assim.
- Abra sem Metro para conferir que o binário é autossuficiente.

## Verificação

Erros fatais de JavaScript são gravados de forma síncrona no aparelho antes do tratamento
original do React Native. Na próxima abertura, o aviso aponta para **Ajustes > Sobre > Último
erro**, com cópia e compartilhamento do relatório. Mensagens desconhecidas e valores variáveis
são ocultados para não guardar conversa ou credenciais. O relatório conserva rota sem parâmetros,
pilha recebida do runtime, componentes e identificação da atualização; uma pilha ofuscada de
Release precisa do source map do mesmo bundle/OTA para chegar às linhas de origem.
O diário do servidor próprio recebe um resumo na reabertura e só confirma o envio após o aceite
do servidor. O relatório completo continua no aparelho, inclusive sem rede.

`npm run typecheck` e `npm test` (em `mobile/`) rodam sob a trava `/tmp/hangar-verificacao.lock`,
compartilhada com as outras sessões da máquina. No repositório, testes e typecheck só rodam
quando pedidos; não contorne a trava.

## Fronteira de manutenção

| Parte | Responsabilidade |
|---|---|
| `packages/core` | APIs e destino por servidor, tipos, parser/histórico, formatação, traduções, agrupamento e transições puras da primeira conversa (`firstConversation.ts`) |
| `mobile` | UI Expo/React Native, teclado, permissões, `AppState`, transporte SSE nativo, persistência e recuperação no aparelho |
| Backend | processos dos providers e entrega ao agente; o app chama o servidor existente |
| Desktop Rust | consome a API do backend e os catálogos `messages/*.json`; mantém UI própria e não importa o core TypeScript |

Os stores nativos coordenam conexões, respostas antigas, SecureStore/MMKV, URIs de anexos
e áudio Expo. Não devem ir para o core só porque têm o mesmo nome que stores do PWA.
A criação já usa as transições puras do core; sua execução e persistência ficam no app.

Duplicações concretas ainda presentes, sem refatoração nesta entrega:

- Classificação de recusa definitiva de SSE em `src/net/sse.ts` e `src/stores/chat.ts`.
- Classificação de input recusado em `src/stores/chat.ts` e `src/stores/newConversation.ts`;
  a criação distingue `409`, portanto as regras não são intercambiáveis.
- Validação de opacidade em `src/stores/aparencia.ts` e `src/theme/unistyles.ts`; o tema
  precisa inicializar antes do store.

Esses exemplos foram conferidos no código mobile/core. A equivalência entre implementações
web e nativa não foi auditada neste lote. A fronteira define onde manter cada integração,
sem afirmar que toda regra compartilhável já foi extraída.

## Limitações conhecidas

- Providers: o mínimo previsto para aceitação é Claude e Codex, com e sem terminal. Pi, omp e
  Kimi não têm aceitação registrada no aparelho.
- Voz/ditado dependem dos recursos de áudio configurados no servidor; sem eles, aparecem
  indisponíveis com o motivo.
- Sem notificação push com o app fechado e sem loja.
- Board, Canvas e navegador embutido são do desktop/PWA, não do app. O app tem terminal real
  para sessões com terminal, com espelho disponível como alternativa explícita.
- Situação de aceitação de cada entrega: `docs/analysis/2026-09-29-mobile-acceptance.md`.
