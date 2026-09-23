---
id: 2026-09-23-orquestrar-patch-retrospectivas
titulo: A skill orquestrar recebe o patch das duas retrospectivas
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.claude/skills/orquestrar/SKILL.md
destrutivo: true
---

As páginas da skill `orquestrar` mudaram: doze regras novas ou reformadas, medidas em duas
execuções reais, e cinco trechos removidos das páginas de papel — três deles vivem no roteador e
dois nas páginas para onde o texto novo aponta (`protecao.md` e `revisor-catalogo.md`). Nenhuma
regra se perdeu.

Quem tem a skill em **symlink** já está lendo a versão nova depois do `git pull` — este passo não
muda nada nessa máquina. Quem está em **cópia** (Git Bash do Windows, onde `ln -s` copia em vez de
linkar) continua lendo a skill antiga até rodar isto: um `git pull` não atualiza cópia. O próprio
instalador avisa disso quando cai nesse caminho.

`destrutivo: true` porque, na máquina em cópia, o instalador **apaga a cópia anterior** da skill
antes de relinkar. Nada além dela é tocado, e o passo pode rodar duas vezes sem efeito diferente —
mas, sendo destrutivo, ele roda pelo botão e não sozinho na subida do backend, que é o que se quer
quando uma sessão pode estar lendo a pasta naquele instante.

**A prova deste passo é de existência, não de conteúdo.** O formato só aceita caminhos que
precisam existir depois, e este patch edita arquivos que já existiam: a prova passa igual com a
skill velha ou com a nova. Fica declarado em vez de escondido — um passo que anuncia a própria
fraqueza vale mais do que um que parece verificado.
