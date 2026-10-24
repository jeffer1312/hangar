# Contas Claude

Uma conta = um config dir (`~/.claude-<nome>`). Criar a conta **não** loga: o OAuth abre navegador
e é interativo, então a sessão sobe deslogada e o `/login` é rodado dentro dela, uma vez.

## O que é compartilhado e o que não é

| Dentro da conta | Forma | Por quê |
|---|---|---|
| `.credentials.json` | real | É o token. Compartilhar faz a renovação de uma sessão derrubar a outra (o token vale ~8h e é reescrito). |
| `.claude.json` | real | Tem o `oauthAccount`. Semeado do teu, sem esse campo — herda permissões por projeto e MCP. |
| `projects/` | real | É onde ficam os `.jsonl` que o painel de custo soma, uma vez por config dir. Compartilhar contaria o mesmo gasto uma vez por conta. |
| `projects/<projeto>/memory/` | atalho | Memória não custa e vale igual em qualquer conta. |
| `settings.json` | cópia | O CLI grava nele sozinho (primeiro boot, `/config`, `/model`); pelo atalho isso apagava a config de todas as contas. A cada abertura o principal é espelhado por cima da cópia (chave de topo inteira; `enabledPlugins` por plugin; o que só a conta tem fica). |
| o resto | atalho | Skills, plugins, hooks, CLAUDE.md — o ambiente é um só. Runtime do CLI por conta (`sessions/`, `telemetry/`, …) não vira atalho. |

Quem mantém isso é a reconciliação do `hangar-server` (`crates/hangar-server/src/accounts/claude_config.rs`):
roda em processo a cada abertura de sessão na conta e na criação dela, e pelo terminal com
`hangar-conta --prep <conta>` (que chama `hangar-server conta`). O que diverge vai para a gaveta
`.drift/` da conta; os `mcpServers` do `~/.claude.json` também são espelhados para a cópia da conta.

## O que isso custa

`claude --resume` numa conta **não** lista conversa de outra conta: o transcript mora na pasta da
conta em que a sessão rodou. Resumir uma conversa de outra conta continua possível pelo app (é
leitura de arquivo, não precisa estar logado nela); continuar aquela conversa dentro da outra
conta, não.

## Por sistema

| Sistema | Estado |
|---|---|
| Linux | Funciona. Medido: config dir vazio faz o CLI responder `Not logged in · Please run /login`. |
| Windows | **Pode não funcionar.** Atalho exige o Modo Desenvolvedor ligado; sem ele a criação falha com mensagem explícita e **sem deixar conta parcial** (rollback), em vez de cair pra cópia (cópia divergiria em silêncio). As duas travas da reconciliação (a da conta e a compartilhada `~/.hangar-contas.lock`) valem também no Windows (trava de arquivo do sistema), com teto de 120 s. |
| macOS | **Provavelmente não isola. Não testado.** A doc oficial escopa o comportamento a Linux e Windows: *"If you've set the `CLAUDE_CONFIG_DIR` environment variable on Linux or Windows, the `.credentials.json` file lives under that directory instead."* No macOS a credencial é do Keychain e a doc não promete separação por config dir — se o item for o mesmo pras duas contas, elas brigam por ele. Teste de um minuto num Mac: criar a conta, rodar `/login` nela e conferir se a conta anterior continua logada. |

## Atalho e descoberta de skills

Há issue **aberto** relatando que skill em diretório symlinkado carrega mas não aparece
(`/skills` vazio, autocomplete sem a entrada): [#14836](https://github.com/anthropics/claude-code/issues/14836),
e o mesmo padrão em #36659 e #25367.

**Não reproduz na v2.1.226** (medido em 10/08/2026, ver a seção de fatos do plano): `plugin list`
idêntico, 177 skills vistas pelo modelo, autocomplete ofereceu uma skill de plugin, e `/skills`
listou `falar · user` vindo do `~/.claude/skills` symlinkado.

Se uma versão futura trouxer o bug de volta, o sintoma é `/skills` vazio numa conta com as skills
ainda funcionando quando invocadas pelo nome. A saída seria espelhar por cópia as pastas onde a
listagem importa, mantendo atalho no resto.

## Deriva

Os atalhos são refeitos a cada abertura de sessão, então pasta nova no `~/.claude` entra na conta
sozinha. Se um arquivo compartilhado tiver sido reescrito dentro de uma conta (quem grava por
tmp+rename substitui o atalho por arquivo comum), a mudança **sobe** pro `~/.claude` e o atalho é
refeito — ela vale nas contas todas, que é o que "compartilhado" quer dizer. Pasta local que
colidir vai pra `.drift/`, que guarda as 3 mais novas — e a `memory/` de um projeto segue a
MESMA regra: memória local de verdade vai pra gaveta com aviso e o atalho é refeito, e atalho de
memória apontando pra projeto que sumiu do `~/.claude` é podado na próxima abertura.

## Um detalhe do `.credentials.json`

Ele também guarda o OAuth dos MCP (tavily, higgsfield). Como é por conta, cada conta nova pede um
`/mcp` de reautenticação desses servidores, uma vez cada.
