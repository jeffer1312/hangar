# Shell Electron

Janela nativa que hospeda o cockpit com a área de trabalho aparecendo atrás.

## Rodar

```bash
cd shell && npm install     # primeira vez
npm start
npm test                    # settings.cjs (leitura/gravação do userData)
```

Ele carrega `http://127.0.0.1:8765` — o backend, que serve a interface. `COCKPIT_URL` aponta
outro endereço. Endereço fora do ar cai no `http://127.0.0.1:8765` antes de perguntar qualquer
coisa; só quando os dois falham é que o app pede o endereço. `Ctrl+Shift+U` reabre essa pergunta.

Pra deixar a janela viva sem ficar presa a um terminal, um serviço transiente do systemd resolve:

```bash
systemd-run --user --collect --unit=cockpit-shell \
  --working-directory=$PWD --setenv=WAYLAND_DISPLAY=$WAYLAND_DISPLAY \
  --setenv=XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR $(which npm) start
systemctl --user stop cockpit-shell     # fechar
```

## Desfoque atrás da janela

A transparência é do app; o **desfoque** é do compositor, e o app não mexe na sua configuração.

**Hyprland** — a classe da janela é sempre `electron` (não dá pra mudar), então a regra casa pelo
título. A sintaxe depende de como sua config está escrita:

```
# config .conf (Hyprland clássico)
windowrulev2 = noblur off, title:^(claude-cockpit)$
```

```lua
-- config .lua (end-4/dots-hyprland e afins)
hl.window_rule({ match = { title = "^(claude-cockpit)$" }, no_blur = false })
```

Medido em 05/08/2026 (Hyprland 0.55.4): **não existe** um rule `blur` pra ligar — só `noblur` pra
desligar, e `blur = true` devolve `unknown field`. Rices que desligam o desfoque de toda janela
(`class = ".*", no_blur = true`, o caso do end-4) exigem que a regra acima venha **depois** da
padrão: no `hyprland.lua` o `custom/` é carregado por último, então é lá que ela vai. E numa config
Lua o `hyprctl keyword` não funciona (`keyword can't work with non-legacy parsers`) — pra testar sem
editar arquivo, use `hyprctl eval`.

A intensidade é global (`decoration:blur:size`/`passes`), não por janela: só dá pra ligar e
desligar. Uma receita calibrada pra barra fina costuma ficar pesada demais numa janela inteira.

**macOS e Windows 11** fazem sozinhos, sem configuração.

**GNOME/KDE** dependem do compositor; sem suporte, a janela fica transparente sem desfoque.

## Fundo

Dentro do shell, a tela de Aparência ganha a opção **Desktop**. Ela usa os mesmos sliders de
Transparência e Solidez das caixas. O slider "Desfoque do fundo" não aparece nesse modo — ali quem
borra é o sistema.
