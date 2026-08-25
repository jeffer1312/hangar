# Passos de atualização

Quando uma versão exige que a máquina de quem já usa faça alguma coisa além de puxar o código —
uma dependência nova, um arquivo que precisa sair da frente, uma pasta que muda de nome —, essa
coisa vira **um arquivo aqui**, no mesmo commit que a exige.

O botão Atualizar do app lê estes arquivos, roda o que ainda não rodou naquela máquina, e nunca
pede nada a quem está usando. Quem lê o formato é `backend/app/atualizacoes.py`.

## Formato

```markdown
---
id: 2026-08-25-exemplo
titulo: Uma frase dizendo o que muda
comando: ./install.sh --update
prova: test -x ~/.local/bin/hangar-send
destrutivo: false
---

O texto que a pessoa lê no changelog. Uma ou duas frases, em português, sobre o que
mudou para ela — não sobre o que o comando faz.
```

| Campo | Para que serve |
|---|---|
| `id` | Chave no registro do que já rodou. Começa com a data para a ordem sair certa. Nunca mude o id de um passo já publicado — a máquina que já o rodou o reconheceria como novo. |
| `titulo` | Obrigatório. Sem ele o arquivo é ignorado (com aviso no log). |
| `comando` | O que rodar. Passa pelo shell, então `&&` e pipe funcionam. O diretório de trabalho é a raiz do repo. |
| `prova` | Como saber que deu certo. Exit code 0 = passou. **Sem prova, "sucesso" quer dizer só "o comando não deu erro"** — e foi assim que um `-Update` chegou a dizer ok com o processo antigo ainda no ar. |
| `destrutivo` | `true` quando o passo apaga ou sobrescreve algo. Passo destrutivo roda pelo botão, mas não roda sozinho na subida do backend. |

## Duas regras

**O passo tem que poder rodar duas vezes.** Ele pode rodar de novo numa máquina que reclonou o
repo. Comando que só funciona uma vez nasce com guarda — o modelo é
`backend/app/migracao_sidecars.py`: destino já existe, para com aviso, nunca funde.

**O passo entra no registro só depois da prova passar.** Se a prova falhar, ele fica pendente e
tenta de novo na próxima — é o contrário de marcar como feito e deixar a máquina sem o efeito.
