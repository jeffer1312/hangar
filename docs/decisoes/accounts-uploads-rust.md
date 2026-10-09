# Contas e anexos no servidor Rust

A migração cobre as contas Claude/Codex e os anexos de sessão. Outros módulos do
backend continuam no Python. O protocolo interno muda nos dois lados juntos.

## Contas

O catálogo, a criação, o preparo, o login, o estado, o cancelamento e o logout são
decididos pelo Rust. Os auxiliares nativos usam a CLI do provedor e a proteção por
conta. A ponte fornece fatos de uso e controla as janelas necessárias ao Claude.
Os destinos secundários Pi/omp são escritos diretamente pelo Rust.

O cache de cotas continua no formato existente, com um escritor e publicação
atômica. O TTL é de cinco minutos; um 429 espera dez minutos, inclusive após
reinício e pedido forçado. O último resultado legível é conservado quando a rede
falha. A assinatura da credencial impede reutilizar a leitura de uma identidade
anterior. Provedores fora desta migração continuam fornecendo leituras pelo Python.

A renovação Claude verifica o uso da conta e mantém exclusividade durante a CLI.
Uma resposta perdida ao abrir ou fechar a janela conserva a guarda e o registro
da operação para limpeza. Uma operação encerrada não pode abrir uma janela tardia.
Não há laço de renovação no Python: sem o Rust, nenhuma conta é renovada.

A redefinição Codex relê a janela de sete dias e só consome crédito quando ela está
em 100%. O UUID é gravado antes do consumo. Retentativas usam a mesma chave; um
resultado definitivo fica persistido e não dispara outro consumo. Depois do
resultado, o cache é invalidado e os limites são relidos.

### Sem reserva Python (09/10/2026)

Contas e cotas não têm mais reserva Python. No modo `python` (sem binário,
`CP_RUST_SERVER=0`, `--reload` ou supervisor que desistiu), as rotas de conta e cota
que o Rust atende respondem 503 com `accounts_need_rust_server`, e os consumidores
internos (criação de sessão, `loop.py`, política do runtime, credenciais, catálogo de
provedores) tratam a ponte ausente como "sem cota, sem login conhecido", sem ler
credencial nem abrir CLI. Manter as duas implementações custava código duplicado e
testes que provavam um caminho que quase nunca roda.

Continua no Python, por ser o que o Rust consome ou o que ele não atende:

- as rotas `/internal` de contas: janela escondida do login e da renovação
  (`ClaudeWindows`), fatos de uso, preparo e invalidação do Codex;
- as cotas de Kimi, CommandCode e OpenCode (`quota_facts`), pedidas pelo Rust;
- os leitores de cota Codex que a transferência de conversa usa;
- as rotas só Python de credenciais: lista, apelido, cookie do OpenCode,
  sincronização nos agentes e remoção de provedor Kimi.

## Anexos

O Rust atende upload, galeria, download e resolução de áudio pelo cofre existente.
O corpo entra em stream, com limite de 100 MiB, temporário privado e publicação
sem sobrescrever um arquivo anterior. Desconexão remove somente o temporário
daquela operação. A galeria e a poda usam a mesma retenção configurada.

O download mantém o descritor validado durante a resposta, suporta Range/If-Range
e dispõe HTML em iframe isolado. SVG/XML mantêm o MIME informado pelo ambiente
e bloqueiam scripts. A admissão de convidados continua no Python e encaminha
os bytes à porta privada do Rust, sem outra implementação do cofre.

Vídeos geram seis quadros distribuídos pela duração e uma faixa de áudio.
Sem duração legível, é tentado um quadro inicial. Cada subprocesso tem prazo,
e os derivados só entram na galeria depois da publicação pelo cofre. Falha de
extração ou transcrição preserva o upload original. A transcrição continua usando
o provedor configurado no Python; `audio_only` evita extração e transcrição duplicadas.

## Verificação

As verificações focadas exercitam cache/429/reinício, concorrência e perda de
resposta do reset, posse dos auxiliares, upload/galeria, limites, Range e isolamento
de documentos. A criação e o login humano devem ser conferidos antes de retirar
o PR de rascunho.
