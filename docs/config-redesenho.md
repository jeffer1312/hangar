# Redesenho das telas de configuração — o que falta

As telas de configuração cresceram por acúmulo: eram 13, divididas em dois grupos (APP e SERVIDOR)
que dizem onde o valor é guardado, não o que ele muda. O mesmo assunto aparecia em telas
diferentes, o mesmo dado aparecia duas vezes com nomes diferentes, e boa parte dos textos de ajuda
descrevia o que o campo *é* em vez do que ele *faz*.

A primeira tela nova (**Voz**) está pronta e na `main`. Este arquivo guarda o que foi decidido e o
que já foi levantado do código, para as sete restantes saírem sem refazer a análise.

## Decisões (04/09/2026, com o usuário)

1. **Público:** qualquer pessoa que baixe o projeto. A tela precisa funcionar para quem nunca a viu:
   caminho padrão pronto, avançado escondido, link para criar chave onde ela é necessária.
2. **Agrupamento por assunto**, não por onde o valor mora. Um aviso discreto dentro da tela diz
   quando um ajuste vale só neste aparelho.
3. **Recurso que depende de chave ausente aparece desligado**, dizendo o que faria e o que falta.
   Fora da configuração (chip, botão, atalho) ele some por completo.
4. **Ligar uma máquina na outra:** endereço e token em campos separados; o app monta a URL, aceita
   `192.168.0.10:8765`, `casa.ts.net` ou o link inteiro colado, e testa antes de salvar. O QR
   continua como atalho.

## De 13 telas para 8

| Nova tela | Absorve | Estado |
|---|---|---|
| Voz | Ditado + a parte de transcrição/leitura de Anexos + o grupo "Ditado e voz" do Avançado | **pronta** |
| Aparência e idioma | Geral + Aparência | a fazer |
| Conversa | ferramentas/tarefas/pensamento/gráfico (hoje em Aparência) + prazo dos anexos + "mostrar o raciocínio do agente" | a fazer |
| Contas e modelos | Contas + Motores de modelo | a fazer |
| Máquinas | Acesso + Servidores + o identificador do servidor, hoje repetido no Avançado | a fazer |
| Notificações | Notificações + a assinatura push e as horas silenciosas de Servidores | a fazer |
| Automação | `automations`, `editor` e `scan_roots`, extraídos do Avançado + Orquestração | a fazer |
| Sobre e diagnóstico | Sobre + Diário de uso + o que só muda pelo `.env` e não é endereço | a fazer |

`Avançado do servidor` deixa de existir quando a última dessas telas nascer: o grupo de voz já saiu,
o raciocínio do agente vai para Conversa, `automations`/`editor`/`scan_roots` vão para Automação, o
identificador vai para Máquinas (editável, uma vez só) e o resto do bloco somente-leitura se divide
— `port`, `lan_bind_ip` e `public_url` ficam em **Máquinas**, junto da história de endereço;
`terminal_panel` fica em Sobre e diagnóstico.

## Regras que valem para todas

- **O texto diz o que o ajuste faz e quando mexer nele**, com o efeito primeiro. Nome de variável de
  ambiente não aparece em rótulo.
- **O caminho padrão fica pronto e visível**; o que só serve para quem troca de provedor fica atrás
  de um "usar outro serviço".
- **Um dado, um lugar.** Valor repetido em duas telas é bug: fica editável onde a pessoa procura e
  some da outra.
- **A semântica de segredo é uma só** ("vazio mantém o valor atual"), com a mesma frase em todas.
- **Todo texto vem de `m.<chave>()`**, com `pt.json` e `en.json` no mesmo commit.
- **Configuração é modal centrado**, e a largura se mede com container query, nunca media query.

## Armadilhas já conferidas no código

Levantadas durante a tela Voz; valem para as próximas.

- **O id de rota `contas` não pode mudar** — `components/QuotaStrip.svelte` e
  `components/DesktopShell.svelte` abrem a configuração direto nele.
- **As rotas são testadas por valor literal** (`lib/configRoute.test.ts`), e
  `components/settings/SettingsModal.test.ts` acha o item do menu pelo texto do título. Toda fusão
  mexe nesses dois arquivos.
- **Rota antiga tem que continuar abrindo alguma coisa.** O mapeamento fica em
  `lib/configRoute.ts` (`RENOMEADAS`), e ele **substitui** o guard de tela desconhecida, não roda
  depois dele. `ditado → voz` já está lá e serve de modelo.
- **`ServerSettings.svelte` é reescrito, não movido.** O `secao` que ele recebe filtra o array
  `CAMPOS` *e* decide blocos escritos à mão fora dele.
- **Arquivo novo não pode ter uma única string crua** — `frontend/i18n-baseline.json` está `{}` e
  `lib/i18nGuard.test.ts` dá limite zero a arquivo ausente da linha de base.
- **Saber se existe chave já é possível:** `runtime_config.estado()` publica
  `campos.<chave>.definido` para cada segredo, e `frontend/src/lib/segredos.svelte.ts` (criado na
  tela Voz) expõe isso ao app inteiro, recarregando quando o servidor ativo muda e depois do Salvar.
  Para campo **não-segredo** o backend calcula `definido = valor is not None`, o que é sempre
  verdadeiro — nesses casos olhe o VALOR, como `segredos.podeLer()` faz com `tts_local_cmd`.
- **A tela de configuração tem componentes prontos para reusar:**
  `components/settings/LinhaConfig.svelte` (uma linha: rótulo, ajuda, tag "editado" e os cinco tipos
  de campo) e o rodapé do Salvar, que só aparece quando há o que salvar e reserva a própria altura.

## Notas por tela

**Contas e modelos.** As duas telas escrevem no MESMO `~/.claude/engines.json`: em Contas se
cadastra Nome/URL/Chave, e para ajustar modelo, subagente, janela de contexto e as nove opções
avançadas *daquela mesma chave* é preciso ir em Motores, sem link entre elas. A lista de Contas já
mistura login OAuth do Claude, chave de API, cota e cookie de painel — a fusão não inventa essa
mistura. **Conta Claude não tem motor:** ela não existe no `engines.json`, então ao abri-la o bloco
de modelo/contexto/avançado não aparece. Em Motores, o "Tool search" continua clicável dizendo
"sem efeito: betas desligados" — é o padrão certo de auto-aviso, e o resto da tela devia segui-lo.

**Máquinas.** Três blocos: esta máquina (identificador, por onde ela responde, QR de pareamento),
máquinas que este servidor alcança (peers, em `backend/peers.json`) e servidores que este aparelho
conhece (a lista do navegador, em `localStorage`). Hoje adicionar máquina é colar um link que já
traz o token dentro (`?token=`) ou ler o QR, e "Registrar servidor como máquina alcançável" exige
digitar a URL com `http://` na mão, senão recusa — dois formatos diferentes para a mesma ideia. É
aqui que entra a decisão 4.

**Notificações.** Os quatro ajustes de "quando avisar" são do servidor; a assinatura push do
navegador e as horas silenciosas moram dentro de Servidores, com uma legenda que existe só para
explicar a própria mistura.

**Sobre e diagnóstico.** O Diário de uso é dado de UMA máquina e fica no grupo APP, sem dizer de
qual — com mais de um servidor cadastrado não dá para saber a origem do log.

**Aparência.** Duas legendas genéricas ("Solidez da folha" e "Força") são reaproveitadas para
efeitos diferentes (cor do tema e leitura sobre foto).

## Ordem sugerida e o ponto de passagem

Um plano por tela, cada um entregando software que funciona sozinho: Máquinas (tem mudança de
comportamento), depois Contas e modelos (a maior), e por fim as quatro que são majoritariamente
mover e reescrever texto.

Não há dependência forte entre elas, mas **Máquinas esvazia o bloco somente-leitura do Avançado**:
quem planejar Sobre e diagnóstico depois precisa partir do que sobrou, senão o identificador volta a
aparecer em dois lugares — o defeito que abriu este trabalho.

## Dívida deixada pela tela Voz

- `LinhaConfig.svelte` usa `--bg-base` no campo de texto, onde a regra pede `--surface-inset`: com
  papel de parede ligado, todo campo de configuração vira retângulo chapado. Veio copiado do
  `ServerSettings` antigo, e vale para as duas telas.
- `.tag` e `.ajuda` estão duplicadas em `LinhaConfig.svelte` e `ServerSettings.svelte` porque as
  duas seguem em uso fora do bloco extraído e o CSS do Svelte é escopado por componente.
- O `VozSettings` não tem a animação do "salvo" que o `ServerSettings` tem.
- Não há teste de componente para `AssistantBubble.svelte` nem para o atalho de teclado do
  `Chat.svelte` (o resto da pasta tem, montando o componente de verdade com `mount()`).
