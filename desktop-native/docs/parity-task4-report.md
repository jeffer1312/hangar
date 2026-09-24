# Task 4: anexos, comandos e ações do compositor

Executor `native-parity-task4-exec` (claude-opus-5-5, medium, conta 200-3). Worktree `hangar-native-desktop`, HEAD 55e0d909 destacado, sem stage/stash/commit. Matriz por capacidade em [`chat-parity.md`](chat-parity.md).

## O que mudou

- `src/composer.rs` (novo): regras puras do compositor portadas do web e do core. `encode_component` (= `encodeURIComponent` do `X-Filename`), montagem do prompt com anexos, legenda canônica (`_cap`), marcadores recebidos (`parseImageMessage`, também para `📎 arquivo:`), caminhos citados (`parseFilePaths`, sem dependência de regex), sugestão de comandos, comando digitado, nome local seguro, tipos que podem abrir. Seis testes compilados.
- `src/api`: `CommandInfo`, `Uploaded`, `UploadFile`; rotas `/commands`, `/upload` (corpo cru, 180 s, queda = incerteza), `/uploads`, leitura autenticada de `/uploads/{f}`, `/file?path=` e `/transcript-image/{id}/{n}` (teto 100 MiB), `/steer {text}`, `interrupt(clear)`.
- `src/delivery.rs`: confirmação pela legenda (o transcript grava só ela) e `take_unconfirmed` para a interrupção devolver a mensagem aceita. Um teste compilado.
- `src/app.rs`: anexos e rascunho por `SessionKey` (servidor + sessão + `jsonl`); upload sequencial amarrado à sessão de origem; seletor do sistema, colar e soltar; sugestões, folha de comandos, confirmação de destrutivo e de troca de rascunho; Tab, Esc e Ctrl+L; interromper com confirmação; "Orientar agora"; anexos recebidos com miniatura, Abrir e Salvar.
- `messages/{pt,en}.json`: só chaves `native_*` novas e `native_send_hint` atualizada.
- `tools/parity_composer_fixture.py` (novo): backend sintético da prova.

## Verificação

```
cd desktop-native && cargo build --locked        → exit 0, 3 avisos já existentes (campos de DTO não lidos)
cd desktop-native && cargo test --locked --no-run → exit 0, "Executable unittests src/main.rs"; nenhum teste executado
```

Falharia se: o compositor não compilasse, ou qualquer teste novo (`composer::tests`, `delivery::tests::attachment_message_confirms_by_caption…`) deixasse de compilar.

## Prova na janela real

Janela própria no workspace 12 do DP-3 (saída física existente), fixture sintética na porta 18794 (e a da Task 3 na 18795 para não regressão), PID/workspace/geometria e posição do cursor conferidos antes de cada clique. Capturas em `~/.hangar/orq/2026-09-23-native-parity/visual/task4-r1/`; vídeo do fluxo em `task4-composer-flow.mp4` no mesmo diretório.

| Caso | Captura | Resultado (log da fixture) |
|---|---|---|
| Anexos recebidos | `02-received-attachments.png`, `02b-received-top.png`, `03-slash-suggest-down.png` | legenda sem marcadores; imagem do cofre, imagem do transcript e imagem citada inline; zip e pdf com ações; HTML só "Salvar". A recusa do caminho: `02-…` é de antes da correção e mostra "Token recusado"; `03-…` em diante mostra o motivo do backend ("caminho não citado nesta conversa") |
| Sugestão `/` + Tab + argumento | `03-slash-suggest-down.png`, `04-tab-completes.png`, `05-compact-sent.png` | destaque acompanha a seta; Tab completa sem tirar o foco; 1 `POST /input {"text":"/compact resumo curto"}` |
| Destrutivo digitado | `06-destructive-confirm.png`, `06b-destructive-cancel.png`, `07-destructive-sent.png` | Enter pede confirmação, Esc cancela sem POST; confirmar envia `/clear` uma vez |
| Tela ausente | `08-other-surface.png` | `/model` avisa a dependência; 0 POST |
| Folha de comandos | `29-command-panel-fixed.png`, `29b-command-search.png`, `33-commands-error.png` | grupos, busca, erro com "tentar novamente"; `/revisar` envia direto |
| Proteger rascunho | `10-replace-confirm.png` | preencher `/compact` sobre texto escrito pede confirmação |
| Seletor do sistema | `11-system-picker.png`, `12-attached.png` | 3 arquivos: PNG com miniatura, TXT, MP3 recusado com motivo |
| Upload 413 | `13-upload-413.png` | 1 upload; motivo no anexo; texto e anexo ficam; 0 `/input` |
| Queda no upload | `14-upload-uncertain.png` | aviso de incerteza; nenhuma repetição automática |
| Troca de sessão | `15-other-session.png`, `15b-back-preserved.png` | rascunho e anexo de cada sessão preservados |
| Resposta atrasada | `16-late-other-session.png`, `16b-late-delivered.png` | upload lento concluído com o Codex aberto; `/input` da sessão certa com `legenda — 📎 imagem: /srv/uploads/…`; anexos saem só dela |
| Orientar | `17-steer-text.png` | `POST /steer {"text":"rascunho do codex"}` |
| Interromper | `18-stop-confirm.png`, `19-stop-returned.png`, `30-stop-returned-note.png` | Esc pede confirmação; mensagem aceita ainda não vista volta ao campo e sai `interrupt?clear=true` |
| Sem terminal aguardando | `20-headless-awaiting.png` | Interromper habilitado |
| Recentes + Tab do terminal | `21-recent.png`, `22-reattached-tab.png` | reanexa pelo download; Tab preenche a sugestão |
| Salvar | `24-save-dialog.png`, `25-saved.png` | diálogo do sistema; bytes idênticos (`cmp`); HTML não abre |
| Colar imagem | `27-pasted.png`, `28-two-images-sent.png` | "colado-4.png"; 2 uploads e 1 `/input` com dois marcadores |
| Não regressão | `31-regress-ask.png`, `32-regress-queue.png` | cartão de pergunta e fila da Task 3 iguais |

Corrigido durante a prova: Ctrl+L sem foco (virou atalho global); recusa 403 exibida como "token recusado" (agora motivo do backend); Tab/setas/Esc seguiam para a navegação de foco (faltava `stop_propagation` na captura); linhas da folha cortadas; aviso de devolução sobrescrito.

## Rodada 2 (parecer `pareceres/task4-r1.md`)

Capturas em `visual/task4-r2/`, todas do binário final (as de um binário intermediário ficaram em `visual/task4-r2/old-binary/` e não valem como prova).

| Bloqueio | Correção | Prova |
|---|---|---|
| 1. Interromper devolvia a mensagem da fila durável e mandava `clear=true` | `take_unconfirmed` só devolve `Delivered` (digitado agora no terminal); `Queued` fica com a fila | `r2-03-queued-stop-confirm`, `r2-04-queued-interrupted`: `delivered:false` → `interrupt?clear=false`, campo vazio, "Na fila" segue. `r2-05-delivered-returned`: `delivered:true` → texto volta, `clear=true`. Teste compilado com o caso `Queued` |
| 2. "Abrir" decidia pelo nome original e gravava nome cortado | `safe_name` corta o radical e preserva a extensão (até 16); `keep_file` recusa abrir se `openable(nome gravado)` for falso (`native_open_refused`); o botão decide pelo nome saneado | `r2-07-recent` (mesma tela): `a…a.html.pdf` (124 caracteres) com Abrir e Salvar, gravado como `…​.pdf`; `b…b.pdf.html` só Salvar. Teste compilado |
| 3. Troca automática de transcript roubava o foco | o foco no compositor saiu de `select` e ficou só no clique da lista | `r2-10-before-rotate`, `r2-11-after-rotate`: com o diálogo aberto, o `jsonl` troca (o rascunho do transcript antigo sai do campo) e o que se digita cai no endereço do diálogo |
| 4. Destrutivo/tela ausente passava com anexo | a checagem de comando roda sempre sobre o texto do campo | `r2-08-clear-with-attachment` (confirmação), `r2-09-model-with-attachment` (aviso); 0 requisições no log |

Divergência da receita, aceita pela árbitra: o passo 4 do bloqueio 2 pedia `openable(safe_name("a…a.html.pdf")) == false`, o que contradiz o passo 1 (preservar a extensão faz o nome terminar em `.pdf`). Vale o comportamento final: o arquivo entregue ao sistema tem sempre extensão passiva. O teste afirma isso e que extensão com mais de 16 caracteres é cortada como antes, com `openable` falso.

Defeito meu da rodada 1 achado agora: o Ctrl+L só funcionava porque a seleção já deixava o campo focado. Sem nada focado (por exemplo, depois de fechar o diálogo de Conexão) a tecla não chegava ao atalho. A raiz da janela passou a ter foco próprio: trata a ação, recebe o foco ao fechar o diálogo, ao conectar e em clique numa área sem foco. Prova: `r2-02-ctrl-l-after-dialog-cancel`.

Lição da revisão aplicada: toda função que decide por `SendOutcome` foi exercitada com `delivered:true` e `delivered:false`. Além da interrupção, o envio com anexo respondido `delivered:false` (`r2-12-attachment-queued`): 1 upload, 1 `/input`, anexo e campo limpos, "Na fila". A fixture ganhou `mode=queued` (só o `/input` consome) e `/control/rotate`.

## Limites e diferenças

- Arrastar e soltar e colar arquivo copiado: implementados, não exercitados.
- Quadros e fala de vídeo: só por teste compilado (a fixture não gera vídeo real).
- A fixture grava na conversa também a mensagem interrompida; no backend real o `clear=true` limpa a entrada do terminal.
- Esc confirma antes de interromper (o web interrompe direto); pedido do plano.
- Marcador do prompt sempre em português: o web usa o texto traduzido, mas `parseImageMessage` e `_cap` só reconhecem `imagem`/`arquivo`.
- Sem prévia/HEIC, progresso em %, áudio→transcrição, visualizador embutido: registrados como pendentes.
- Prova só em fixture sintética; nenhuma sessão real recebeu envio.

## Incidentes de ambiente

- Um clique de prova caiu fora da janela: o `ydotool` absoluto usa escala 1,2. Caiu na borda inferior do eDP-1, sem efeito; o DP-3 voltou ao ws9 na sequência. O script passou a converter a escala e conferir a posição do cursor (±3 px) e o workspace visível antes de cada clique. Árbitra avisada.
- "Abrir" no PDF usou o programa padrão da máquina (Chrome dele) e criou uma aba; fechei só essa aba, conferindo foco e título antes.
- Colar exigiu a área de transferência: a imagem que ele tinha foi salva e restaurada (SHA-256 igual antes e depois).
- Ao fim da rodada 2: janela de teste e fixture encerradas por PID, porta 18794 livre, DP-3 no ws9, demo 4190872 intacta, janela final relançada com o binário final na unidade `hangar-native-parity-task4-final-20260924` (ws12, sem token salvo).
