# Parte 2C — observação de terminais no Rust

## Contrato

Uma conexão `tmux -C attach-session -E -f read-only,ignore-size` por sessão observada, com contagem dos consumidores internos. Não envia teclas, resize nem respostas ANSI. O alvo vem de `tmux._pane_target`, jamais de um pane ativo escolhido novamente.

Captura tipada aceita nome, provider (`claude`/`codex`), binding da conversa, alvo resolvido, consumidor, quantidade de histórico (200 padrão), cores e join. Resposta conserva binding, início da leitura e texto exato. Frame, UTF-8, tamanho, capacidade, EOF ou timeout inválidos são erro; nunca pane vazio. A grade `alacritty_terminal =0.26.0` acompanha saída e checkpoints autoritativos sem escrever de volta. A captura tmux conserva a verdade sobre histórico e wrap.

Um listener TCP interno efêmero `127.0.0.1:0`, no mesmo processo Rust e independente do bind público (inclusive IP específico da LAN), serve somente o endpoint de observação. A saúde informa seu endereço real; o Supervisor valida loopback e porta antes de habilitar a ponte. O endpoint, protegido pelo segredo interno em memória, recebe adquirir/capturar/liberar/processar. Nada de comando livre. Guarda limites e descarte de consumidores abandonados; último consumidor libera o processo. Python não conserva credencial no ambiente global e apaga a ponte quando o filho sai. Windows nem tenta controle.

## Estado e prévia

Portar as funções puras Claude e os seletores do Codex. Reducer recebe pane, memória temporal e fatos estruturados: pergunta do hook/plugin, estado plugin, estado registro/hook, statusline sidecar. Devolve estado, label, pergunta/opções, memória atualizada e atributos de tela. Preservar constantes 3/4/8, animação, ordem das fontes e menus imediatos. O Python mantém permissão, loop, shells, deduplicação, drain e publicação compartilhada.

Prévia sidecar é primeira fonte: `None` cai no pane, vazio responde vazio. Extração Rust só para Claude, sem alterar cadência, geração, markdown/full e reconciliação do transcript. Codex mantém app-server assinado da thread e preview por push; seu monitor terminal mantém o observador compartilhado vivo durante o consumo, inclusive ocioso, mas a captura não substitui seu estado. Sem terminal nunca adquire observador tmux.

## Aceitação

- Fixtures `pane_*.txt` relevantes e entradas sintéticas com saída Python/Rust idêntica, inclusive sequências temporais.
- Controle isolado: duas leituras usam o mesmo pid; alvo exato, vazias, `%`, UTF-8, ANSI/join/histórico, alternate screen, resize externo, queda e liberação.
- Guard de presença não conta cliente de controle e falha continua conservadora.
- Ponte prova reserva, binding trocado em voo, timeout/erro e descarte após `/clear`.
- Testes focados, revisão independente por Task, commits descritivos seletivos, nenhum push/instalador/restart.
