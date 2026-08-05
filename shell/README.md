# Shell Electron

Janela nativa que hospeda o cockpit com a área de trabalho aparecendo atrás.

## Rodar

```bash
cd shell && npm install     # primeira vez
npm start
npm test                    # settings.cjs (leitura/gravação do userData)
```

Ele carrega `http://127.0.0.1:8765` — o backend, que serve a interface. `COCKPIT_URL` aponta
outro endereço. Se a página não carregar, o app pergunta; `Ctrl+Shift+U` reabre essa pergunta.

## Desfoque atrás da janela

A transparência é do app; o **desfoque** é do compositor, e o app não mexe na sua configuração.

**Hyprland** — a classe da janela é sempre `electron` (não dá pra mudar), então a regra casa pelo
título:

```
windowrulev2 = blur, title:^(claude-cockpit)$
```

**macOS e Windows 11** fazem sozinhos, sem configuração.

**GNOME/KDE** dependem do compositor; sem suporte, a janela fica transparente sem desfoque.

## Fundo

Dentro do shell, a tela de Aparência ganha a opção **Desktop**. Ela usa os mesmos sliders de
Transparência e Solidez das caixas. O slider "Desfoque do fundo" não aparece nesse modo — ali quem
borra é o sistema.
