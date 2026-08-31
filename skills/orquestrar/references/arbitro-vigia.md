# Árbitro — a vigia e o silêncio

Esta página é do **momento em que alguém deveria estar trabalhando e você não recebeu nada**: como
armar a vigia, o que fazer quando ela dispara, e como distinguir sessão parada de sessão morta.

Você a lê ao armar a vigia (uma vez, no lançamento) e quando um alarme chega. O resto do tempo ela
não é sua — o ciclo normal está em `arbitro.md`.

## Ociosidade — o sinal de que alguma coisa não chegou

Você sempre sabe **quem deve trabalho**: o executor da Task liberada, ou o revisor do commit que
você mandou. Enquanto o dono da vez está `working`, não existe nada a fazer — Task inteira demora, e
cutucar quem está trabalhando é ruído.

O sinal é o inverso: **o dono da vez está `idle` e você não recebeu nada.** Só há três causas, e as
três se resolvem sem perguntar a ninguém:

1. **A mensagem não chegou** (fila, sessão reiniciada) → reenvie uma vez, dizendo que é reenvio.
2. **A resposta foi produzida e não enviada** — a sessão terminou o parecer e morreu antes do recado,
   ou o envio falhou → **leia o transcript dela** (`~/.claude*/projects/<cwd-sanitizado>/<uuid>.jsonl`,
   o mais recente, mensagens `type: "assistant"`; a última costuma ser exatamente o que faltou).
3. **A sessão sumiu** → seção abaixo: abre outra e segue.

**Você não reenvia antes de olhar o disco.** O arquivo dele pode já estar lá, e o transcript quase
sempre tem o texto inteiro. Ler custa um `cat`; reenviar custa um turno da sessão paga e pode
chegar duplicado.

**E antes de culpar o canal, olhe o pane do destinatário.** Um assistente de primeira execução
aberto na sessão dela recusa toda digitação, e o backend reporta isso como "sessão indisponível" —
que parece fila quebrada e não é.

**Todo mundo do time ocioso ao mesmo tempo é o alarme mais forte que existe**, porque em operação
normal alguém está sempre com a bola. Chegou nesse estado sem ter fechado uma Task: alguma coisa
não chegou.

Não fique olhando, e **não pergunte "e aí?"**: as duas coisas gastam turno seu, que é o token mais
caro da mesa. Deixe uma **vigia em segundo plano** — um laço de shell, não um turno de modelo — que
consulta o estado das sessões e te acorda quando o dono da vez fica ocioso. Use o script que já vem
com a skill:

```bash
systemd-run --user --unit=vigia-<gid> --property=Restart=always --property=RestartSec=20 \
  "$SKILL/scripts/vigia.sh" <sessao> [sessao...] <arbitro> -m 5 \
  -d ~/.hangar/orq/<data>-<gid>/registro.md
```

Três detalhes do comando, e cada um já custou o alarme inteiro:

- **Os minutos vão por flag (`-m 5`), nunca como número solto no fim** — com mais de três sessões o
  número posicional vira NOME de sessão, e os alarmes passam a ser entregues a uma sessão chamada
  "5" enquanto o grupo para.
- **Serviço, não `setsid nohup … &`**: o processo em segundo plano do turno **não sobrevive a
  ele** — some do `ps`, log vazio, sem erro nenhum. Vigia que morre junto com você não cobre o caso
  que ela existe pra cobrir, que é justamente você morrer. E `Restart=always` é a outra metade: sem
  ele, a unidade que cair deixa o trabalho sem rede e você descobre horas depois.
- **O último nome é sempre o árbitro**, e o `-d` aponta o registro: a vigia te cobra se ele ficar
  60 min sem escrita.

**A vigia cobre quem tem a BOLA agora, mais você — e mais ninguém.** A lista de sessões do comando é
o estado da vez, não a tabela do contrato: sessão que ainda não abriu, sessão aposentada e sessão
**parada por ordem sua** ficam de fora, e você **reescreve o comando a cada passe de bola** — ao
liberar Task, ao mandar commit pro revisor. Isso inclui o executor: depois de um `REPROVA` a bola
passa do revisor pra ele **sem você ver**, e é o desenho. Num lote paralelo "quem tem a bola" é
todos os escritores, porque ali todos têm — uma vigia só, com todos eles dentro:

```bash
systemd-run --user --unit=vigia-<gid> --property=Restart=always --property=RestartSec=20 \
  "$SKILL/scripts/vigia.sh" t1 t2 t3 review review2 arbitro -m 10 \
  -d ~/.hangar/orq/<data>-<gid>/registro.md
```

**Ninguém com a bola = vigia desarmada — e a bola com o USUÁRIO também é ninguém com a bola.** Time
sem trabalho (tudo aprovado, esperando decisão do usuário) com a vigia viva só produz alarme falso e
cutucão em sessão paga. Desarme **antes** de perguntar ao usuário, e rearme quando a resposta
chegar. Árbitro em `awaiting_input` esperando resposta humana não é árbitro caído — é o estado
legítimo de quem já entregou a decisão; a vigia não distingue os dois, e quem distingue é você, que
é justamente quem ela acorda.

**Alarme falso tem uma família só, e ela é grande:** sessão parada por ordem sua lida como sessão
quebrada — a que ainda não abriu, a que já entregou, a que espera veredito. A vigia não distingue
"parada porque acabou" de "parada porque quebrou". **Cutucão em sessão parada de propósito não é só
ruído: é um turno pago**, e a sessão cutucada divide árvore com quem está medindo os portões.

E o comando da vigia **não** vai no arquivo de regras com a lista de nomes: vai a forma. Lista de
sessões escrita num arquivo envelhece entre a escrita e a leitura, que é a mesma razão de o estado da
vez morar no kick-off.

Ela consulta a cada 60s e acorda você depois de N leituras paradas seguidas. Três coisas nela não
são detalhe de implementação — são o que a faz funcionar, e cada uma custou uma falha real:

**1. Ela vigia TODAS, incluindo VOCÊ.** Vigiar só o par deixa de fora o modo de falha que ninguém
olhava: o juiz cair. Árbitro derrubado por erro de provedor de madrugada já parou um time inteiro
por 2h30 com o relato preso na fila — e do lado de dentro isso é invisível, porque o turno seguinte
parece continuar de onde o anterior parou.

**2. Ela acorda por `hangar-send --tmux`, não por `echo`.** Um `echo` de processo de fundo só vira
notificação se o turno já estiver **vivo**; com ele morto, a vigia grita pro vazio. O `hangar-send`
entra como **prompt** e reanima turno morto. O `--tmux` é obrigatório: o `hangar-send` normal
**recusa** falar com sessão Claude da mesma máquina (rc=3, "use SendMessage"), e script de shell não
tem `SendMessage`.

**3. Ela dispara quando o DONO DA VEZ para — não quando todos param.** Árbitro parado com alguém
trabalhando é o estado **normal**. `sumiu` conta como parado: sessão morta também não trabalha. Duas
exceções avisam na hora, sem esperar o silêncio: sessão **travada** (diz `working` e não produz
evento há 10 min) e sessão **sem cota**.

> A condição "todas paradas" foi tentada e é cega justamente para o caso que a vigia cobre: com o
> árbitro na lista, ele conversando com o usuário conta como "trabalhando" e mascara um executor
> morto — já custou 30 minutos de silêncio que quem percebeu foi o usuário. Enquanto o script não
> separar os dois papéis, o paliativo é **tirar o árbitro da lista sempre que houver executor com a
> bola** e devolvê-lo quando ninguém tiver.

**A vigia PERGUNTA; ela não manda parar.** É um laço de shell que lê dois números — quanto tempo
sem evento, e se o último comando repetiu — e a partir disso ela **não sabe** se a sessão está
travada ou trabalhando bem. Alarme escrito no imperativo faz o destinatário obedecer sem checar,
porque a mensagem chega pela mesma porta das ordens de verdade. Medido em 24–28/08/2026: **três
alarmes falsos num dia**, e um deles mandou o executor PARAR no meio de 44 minutos de trabalho
legítimo.

A prova de que a forma do texto decide está dentro do próprio script: o aviso de ociosidade é
**condicional** ("se ela re-checa a mesma condição, …") e ninguém nunca parou por causa dele; o de
loop era **imperativo** ("PARE de re-checar") e quase abortou uma Task boa. Toda mensagem que a
vigia manda a uma sessão do time é uma **pergunta com a evidência junto**:

> `[vigia] Você repete o mesmo comando há ~N min. Isso é espera por condição externa? Se for,
> o teto já estourou — reporte ao árbitro o que espera. Se você está trabalhando, ignore.`

A que vai para **você** (árbitro) pode ser afirmativa: você é quem decide, e é o único que pode
distinguir sessão parada de propósito de sessão quebrada. Ordem de parar continua existindo — só
que ela vem de você, depois de olhar, e não de um contador de minutos.

**A prova de que ela funciona é o alarme sintético CHEGAR.** Ao armar, a vigia dispara sozinha um
`[vigia] ARMADA ...` para você, **pelo mesmo caminho dos alarmes reais** — se esse prompt chegou na
sua sessão, o canal está provado; se a unidade subiu e ele não chegou em 2 minutos, o canal está
quebrado e "active" não vale nada. Teste digitado à mão não conta: em 17/08/2026 ele "provou" duas
vezes um caminho que não era o quebrado, enquanto 10 alarmes reais iam pro vazio.

**Confirmar que ela subiu NÃO é confirmar que ela vive.** `systemctl --user is-active` logo depois do
`systemd-run` responde `active` porque a unidade acabou de nascer — não porque ela está lendo a API.
Medido em 17/08/2026: uma vigia ficou `active` por horas, **sem uma linha de log**, enquanto quatro
executores paravam por cota e ninguém era avisado; quem percebeu foi o usuário. As duas confirmações
que valem, e são baratas:

```bash
journalctl --user -u vigia-<gid> --since "-3min"   # tem que estar SEM erro repetido a cada ciclo
systemctl --user show vigia-<gid> -p ActiveState -p MainPID
```

Espere **um ciclo inteiro** (o intervalo é de 60s) antes de dar por confirmada — e erro repetido a
cada ciclo no journal é vigia quebrada, mesmo que a mensagem não se anuncie como erro dela.

**Rearme a vigia toda vez que passar a bola** — ao liberar Task, ao mandar commit pro revisor. Vigia
vencida e não rearmada é silêncio que ninguém percebe. **E mate a vigia antiga ao aposentar uma
sessão**, senão ela lê "sumiu" como parado e te acorda pra alarme falso. Uma vigia viva por vez,
apontando pro par da vez.

Recado de sessão chega como prompt e já te acorda sozinho: a vigia é a **rede** pro caso de o recado
não vir, não o caminho normal.

## Modo noturno — três pré-condições, ou você não dorme o grupo

Deixar o time virar a noite sem usuário é legítimo — com três coisas provadas ANTES, porque de
madrugada não há quem descubra o que você não previu. Medido em 16–17/08/2026: a cota do provedor
dos executores estourou às 23:35, os 4 morreram no mesmo minuto, a vigia estava `inactive` — e
quem descobriu foi o usuário, às 05:56, 6h21 depois.

1. **Vigia provada** — não `active`: o alarme sintético que ela mesma dispara ao armar chegou como
   prompt na sua sessão (ver a seção da vigia).
2. **Cota conferida** — a cota restante de cada provedor do time contra o consumo médio por Task
   já medido neste trabalho. Não cobre a noite → não largue.
3. **Fallback válido** — o plano B de provedor que o contrato autorizou por escrito ainda existe.

Qualquer uma falhando: **pare no fim da Task corrente e acorde o usuário ANTES de dormir** — uma
pergunta às 23h custa uma resposta; a falta dela custou 3 intervenções de madrugada.

## Sessão que morre não é caso de investigação

Sessão do time desaparecida (some do `hangar-send --list` e do tmux) sem você ter mandado fechar: **abra
outra e siga**. Autonomia é isso — o trabalho não pode parar porque uma sessão caiu.

O usuário fecha sessão quando quer, a máquina reinicia, o processo morre. Nada disso é incidente;
todos têm o mesmo conserto. Perseguir a causa custa turnos, interrompe o usuário com um alarme falso
e não devolve a sessão. Já aconteceu aqui: um árbitro interrogou o executor sobre "qual `pkill` você
rodou" quando o usuário simplesmente tinha fechado a janela.

O que fazer, em ordem, sem perguntar a ninguém:

1. **Leia o transcript da sessão morta** (`~/.claude*/projects/<cwd-sanitizado>/<uuid>.jsonl`, o mais
   recente, mensagens `type: "assistant"`). Ela pode ter **produzido** o parecer ou o reporte e
   morrido antes de enviar — nesse caso o trabalho não se perdeu e você nem precisa refazer.
   **E olhe o pane antes de pedir qualquer coisa de novo**: `tmux capture-pane -p -t "=<nome>:" -S -200`.
   Com o canal de saída morrendo (acontece em provedor instável), o reporte inteiro fica **na tela**,
   completo, sem nunca ter saído. Medido em 22/08/2026: um reporte de Task com prints descritos um a um
   estava ali o tempo todo; quem percebeu que a sessão "não conseguia enviar" foi o usuário.
2. **Abra a substituta** pela receita de sempre (criar → provar → pedido em arquivo → conferir a
   entrega), com o kick-off completo: papel, HEAD esperado, intocáveis literais, contrato, plano, e o
   commit ou a receita da vez.
3. **Registre no contrato** em uma linha: qual sessão sumiu, o que foi recuperado do transcript e
   quem assumiu.

Só vire caso de investigação se o **repo** também estiver estranho — árvore suja que ninguém explica,
commit que ninguém reportou, intocável mexido. Aí o assunto é o repo, não a sessão.

