# Testar um conector

## Host isolado, sem servidor

`--registry <arquivo>` cria um host só para o comando, com catálogo próprio. Não sobe backend, não
adota sessões, não instala hooks e não abre porta. Ao fim do comando, o processo do conector é
encerrado.

```bash
cd crates
cargo build -p hangar-server
cargo build -p hangar-help-connector --bin hangar-help-test-connector
REG=$(mktemp -d)/connectors.json
target/debug/hangar-server connectors --registry "$REG" add ../examples/connectors/work_tracking.json
target/debug/hangar-server connectors --registry "$REG" tickets js-demo --limit 2
target/debug/hangar-server connectors --registry "$REG" ticket js-demo c0ffee-01
```

O exemplo em JavaScript precisa de `node` no PATH; o host e o conector Rust do Help não precisam de
Python nem de Node.

## Conector de teste do Help

Só leitura (`GET /api/eu`, `GET /api/pms/pagina`, `GET /api/pms/{chave}`), sem seguir
redirecionamento. Mapeamento: `chave` é `id` e `key`; `resumo` é `title`; `descricao` é
`description_markdown`; a categoria do status vira `todo`/`in_progress`/`done`; `responsavel` vira
`{id: login, name: nome}`, e o login é o que `assignee` aceita de volta (`me` vira o login de quem
consulta); `project` é sempre `null`.

Credencial: o token que o plugin Promedico já guardou, relido a cada consulta, na mesma ordem do
plugin: `credenciais.json` das pastas do plugin (`PMEDICO_CONFIG_DIR` quando definida; senão
`~/.config/pmedico`, `~/.local/share/pmedico` e `~/.pmedico`, ou `%USERPROFILE%\.pmedico`,
`%APPDATA%\pmedico` e `%LOCALAPPDATA%\pmedico` no Windows; vence o valor mais recente), depois o bloco
`env` do `settings.json` do Claude Code, depois a variável `HELP_TOKEN`. O token não vai ao
manifesto nem a argumento. Sem token, ou token recusado, a resposta é `auth_required`: refaça o
login do plugin na mesma máquina e consulte de novo, sem reiniciar nada.

Manifesto, numa pasta ao lado do binário (o caminho relativo parte da pasta do manifesto):

```json
{
  "manifest_version": 1,
  "id": "help-test",
  "name": "Help de teste",
  "version": "0.1.0",
  "protocol_version": 1,
  "capabilities": { "work_tracking": 1 },
  "command": ["./hangar-help-test-connector"]
}
```

Argumentos opcionais do binário: `--id` (padrão `help-test`), `--help-url` (padrão
`PMEDICO_HELP_URL` ou o Help de produção; HTTP só no loopback), `--config-dir` e
`--claude-config-dir`.

## Em outra máquina

1. Na máquina de teste, com o checkout na branch e o Hangar parado ou não, compile para o sistema
   dela: `cargo build --release -p hangar-server` e, para usar a casca de teste,
   `cargo build --release -p hangar-help-connector --bin hangar-help-test-connector`
   em `crates/`.
2. Confira o login do plugin Promedico **naquela máquina**. Token não se copia entre máquinas.
3. Prove primeiro no host isolado (seção acima), com o manifesto do Help: `tickets help-test
   --assignee me --limit 5`, a página seguinte pelo `next_cursor` e `ticket help-test <chave>`.
4. Só com o servidor da branch instalado e reiniciado: `hangar-server connectors add <manifesto>` e
   as mesmas consultas, agora pelo servidor (ou `GET /api/connectors/help-test/tickets` com a senha
   do dono).
5. Volta: `hangar-server connectors remove help-test`, apagar `~/.hangar/connectors.json` se quiser
   zerar, e voltar o checkout para a branch anterior pelo mesmo caminho usado para trocar. O catálogo
   não altera nada fora dele.

## Perfil do Help fornecido pelo Hangar

O servidor distribuído inclui `hangar-server help-connector`, que oferece leitura e o catálogo de
ações do plugin Promedico. O host reconhece o executável legado `hangar-help-test-connector` com
os argumentos documentados e deriva o perfil efetivo em memória, usando o executável do servidor
atual. Preserva ID, argumentos, pasta, prompt de início e estado ligado ou desligado.

O arquivo cadastrado não é reescrito: voltar ao servidor anterior recupera o comando original.
Executáveis personalizados, argumentos desconhecidos e capacidades fora desse perfil continuam
com o manifesto cadastrado. Nenhuma credencial vai para o catálogo ou para a linha de comando.
