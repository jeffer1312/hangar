# Referência Python do navegador remoto

As saídas foram obtidas executando a implementação do commit
`ba2db1021ab7fb801eca50a50f9509dcb825ac7e`, anterior à migração destas rotas.
Os geradores exportam essa fonte com Git e isolam HOME, configuração e diretório corrente.
Não iniciam o backend real nem usam contas ou navegadores das sessões do usuário.

- `url-commands.json`: handler original da GET, autenticação/middleware originais e cliente
  `navshell` contra servidores HTTP temporários. O handler é extraído com seu decorador por AST,
  evitando o lifespan do app principal.
- `transport.json`: `nav_ws` original em uma rota WebSocket, com desktop de ensaio que atende
  `/cmd` e `/cdp` no loopback. Os quadros JPEG vêm da captura real congelada abaixo.
- `chromium.json`: respostas CDP, quadro, screenshot e entrada medidos em um Chromium com
  perfil temporário, usando o cliente `_Cdp` original. O teste de texto insere acento/emoji e
  confirma que Backspace remove o emoji inteiro. Não representa o app nativo ou WebView2.

Cada corpus registra commit, comando, versão, entradas, saídas, costura exercitada e limites.
IDs do inventário identificam a área exercitada; não significam fechamento automático do item.
As normalizações de caminhos, portas e valores aleatórios estão declaradas nos próprios arquivos.
Os JSON esperados não devem ser editados à mão nem substituídos pela saída futura do Rust.

## Executar na raiz do repositório

```bash
backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/generate_url_commands.py
backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/generate_transport.py --check
```

O primeiro comando regenera o corpus Python; para comparar sem sobrescrever a referência,
use `--output <arquivo-temporário>` e compare os bytes. O segundo compara a regeneração do
transporte com a referência congelada. Só retire `--check` quando for intencional publicar uma
nova caracterização Python, preservando a origem e explicando a mudança.

Uma nova captura externa usa um executável instalado e um perfil novo:

```bash
backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/capture_chromium.py --browser <chrome-headless-shell> --output <nova-captura.json>
```

Metadados temporais, IDs e pixels dessa captura são dados medidos. Não se espera que outra
execução do Chromium produza bytes idênticos. O transporte usa a captura congelada como entrada
externa fixa; não sobrescreva `chromium.json` ao conferir a regeneração dos outros corpora.

Esta referência foi executada no Linux. Não prova a migração Rust, o porteiro Rust, Electron,
WebView2, encerramento/reabertura completos do Hangar ou desempenho comparado. A aprovação das
diferenças e as provas restantes ficam nos documentos da migração.
