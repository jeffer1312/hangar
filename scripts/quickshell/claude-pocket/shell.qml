//@ pragma UseQApplication
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Hyprland

// Painel "control center" do claude-pocket, organizado em ABAS por um trilho à esquerda
// (mesmo padrão do control center do rice): Sessões / Projetos / Ajustes — pensado pra crescer
// por páginas, não por seções empilhadas. Instância SEPARADA do quickshell (qs -c claude-pocket)
// de propósito: não toca em nenhum arquivo do rice, então update do dots-hyprland não apaga
// isto e um bug daqui não derruba a barra.
ShellRoot {
    id: shellRoot

    property bool open: false
    // Aba ativa do trilho. Persiste enquanto o processo vive — reabrir volta onde estava.
    property string tab: "sessoes"
    // Sessão com menu de contexto aberto (null = nenhum) e o ponto onde ele abre. Um só por vez.
    property var menuSession: null
    property real menuX: 0
    property real menuY: 0
    // Mensagem de erro do último toggle de servidor ("" = sem erro).
    property string peerResult: ""

    // Projetos por estado: vivo (ou falhado) ganha card completo com log; parado vira tile
    // compacto na grade — mesmo padrão do control center do rice, onde só o que toca/importa
    // é card grande e os toggles são pílulas.
    readonly property var activeProjects: Sessions.projects.filter(p => p.state !== "stopped")
    readonly property var stoppedProjects: Sessions.projects.filter(p => p.state === "stopped")
    // Tile com start em andamento ("" = nenhum) e erro da última ação da grade.
    property string startingProject: ""
    property string projectError: ""

    function startProject(name: string): void {
        if (projStartProc.running)
            return;
        shellRoot.projectError = "";
        shellRoot.startingProject = name;
        projStartProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", name, "project", "start"];
        projStartProc.running = true;
    }

    Process {
        id: projStartProc
        stdout: StdioCollector {
            onStreamFinished: {
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    r = {
                        ok: false,
                        message: "resposta inválida do cp-panel-action"
                    };
                }
                // Mesmo contrato do peer: falha vira texto na aba, nunca morre muda num botão.
                shellRoot.projectError = r.ok ? "" : (r.message ?? "falhou");
                shellRoot.startingProject = "";
                Sessions.refresh();
            }
        }
    }

    // Cadastro de projetos (aba Ajustes): erro da última ação de add/del.
    property string projActionError: ""
    // Fila de cadastros pendentes da varredura (scanAddChecked): um Process por vez, então o
    // próximo da fila só dispara quando o anterior termina (ver onStreamFinished abaixo).
    property var scanQueue: []

    Process {
        id: projActionProc
        stdout: StdioCollector {
            onStreamFinished: {
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    r = {
                        ok: false,
                        message: "resposta inválida do cp-panel-action"
                    };
                }
                // ACUMULA o erro (não sobrescreve): num lote (scanAddChecked) o erro de um item do
                // meio não pode ser apagado pelo sucesso do item seguinte — senão o usuário vê
                // "tudo ok" com uma pasta que nunca entrou. Sucesso NÃO limpa aqui: a limpeza é no
                // início de cada ação (projAdd/projDel), pra o erro sobreviver até o fim do lote.
                if (!r.ok)
                    shellRoot.projActionError = (shellRoot.projActionError ? shellRoot.projActionError + " | " : "") + (r.message ?? "falhou");
                Sessions.refresh();
                // Próximo da fila: dispara o Process DIRETO (não via projAdd, que limparia o erro
                // acumulado do lote).
                if (shellRoot.scanQueue.length > 0) {
                    const next = shellRoot.scanQueue[0];
                    shellRoot.scanQueue = shellRoot.scanQueue.slice(1);
                    projActionProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", next.name, "project-add", next.cwd, next.command];
                    projActionProc.running = true;
                }
            }
        }
    }

    // add: rest = [cwd, command, port?]; del: sem rest. Um Process serializa os cliques.
    function projAdd(name, cwd, command, port): void {
        if (projActionProc.running)
            return;
        shellRoot.projActionError = "";
        let cmd = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", name, "project-add", cwd, command];
        if (port && String(port).trim() !== "")
            cmd.push(String(port).trim());
        projActionProc.command = cmd;
        projActionProc.running = true;
    }
    function projDel(name): void {
        if (projActionProc.running)
            return;
        shellRoot.projActionError = "";
        projActionProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", name, "project-del"];
        projActionProc.running = true;
    }

    // Importar de outra máquina: candidatos do peer escolhido (✓ casado / ✗ sem pasta local).
    property var importCandidates: []
    property string importServer: ""

    Process {
        id: importProc
        stdout: StdioCollector {
            onStreamFinished: {
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    r = {
                        ok: false
                    };
                }
                shellRoot.importCandidates = (r.ok && r.candidates) ? r.candidates : [];
                shellRoot.projActionError = r.ok ? "" : (r.message ?? "import falhou");
            }
        }
    }

    function importFrom(server): void {
        if (importProc.running)
            return;
        shellRoot.importServer = server;
        shellRoot.projActionError = "";
        importProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", server, "import-candidates"];
        importProc.running = true;
    }

    Process {
        id: pickProc
        stdout: StdioCollector {
            onStreamFinished: {
                // Reabre o painel escondido em pickFolder() — tanto se escolheu quanto se cancelou.
                shellRoot.open = true;
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    r = {
                        ok: false,
                        message: "resposta inválida do cp-panel-action"
                    };
                }
                if (r.ok && r.path) {
                    fCwd.text = r.path;
                    // Basename da pasta como nome do projeto por padrão — economiza digitação,
                    // mas só se o campo ainda estiver vazio (não pisa num nome já digitado).
                    if (fName.text.trim() === "")
                        fName.text = r.path.split("/").pop();
                } else if (!r.ok) {
                    shellRoot.projActionError = r.message ?? "falhou";
                }
            }
        }
    }

    // Dispara a caixa de diálogo NATIVA (kdialog/zenity) do sistema pra escolher a pasta do
    // projeto — substitui o navegador de pastas embutido no painel.
    function pickFolder(): void {
        // Guard cruzado: se um kdialog da varredura já está aberto, não dispara outro — dois
        // pickers concorrentes recobririam um ao outro ao reabrir o painel.
        if (pickProc.running || scanPickProc.running || scanProc.running)
            return;
        // Esconde o painel enquanto o kdialog abre: o painel é layer-shell (fica ACIMA das janelas
        // normais), então sem isto o diálogo nativo abre ATRÁS dele (e o botão Cancelar fica coberto).
        // onStreamFinished reabre o painel quando a seleção volta.
        shellRoot.open = false;
        pickProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", "x", "pick-folder", fCwd.text];
        pickProc.running = true;
    }

    // Varredura de pasta pai (aba Ajustes, "Varrer uma pasta"): escolhe UMA pasta pai e cadastra
    // várias subpastas de uma vez, com comando auto-detectado — soma ao form manual acima.
    property var scanCandidates: []

    Process {
        id: scanPickProc
        stdout: StdioCollector {
            onStreamFinished: {
                // Reabre o painel escondido em scanStart() — mesmo motivo do pickFolder.
                shellRoot.open = true;
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    r = {
                        ok: false,
                        message: "resposta inválida do cp-panel-action"
                    };
                }
                if (r.ok && r.path) {
                    shellRoot.projActionError = "";
                    scanProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", "x", "scan-folder", r.path];
                    scanProc.running = true;
                }
                // cancelou: sem path, não faz nada (igual pickFolder).
            }
        }
    }

    Process {
        id: scanProc
        stdout: StdioCollector {
            onStreamFinished: {
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    r = {
                        ok: false,
                        message: "resposta inválida do cp-panel-action"
                    };
                }
                if (r.ok) {
                    // já desmarca os "already" (não recadastra sem querer o que já existe).
                    shellRoot.scanCandidates = (r.entries ?? []).map(e => Object.assign({}, e, {
                                checked: !e.already
                            }));
                    shellRoot.projActionError = "";
                } else {
                    shellRoot.scanCandidates = [];
                    shellRoot.projActionError = r.message ?? "varredura falhou";
                }
            }
        }
    }

    // Dispara o kdialog pra escolher a pasta PAI e depois varre as subpastas dela.
    function scanStart(): void {
        if (scanPickProc.running || scanProc.running || pickProc.running)
            return;
        shellRoot.projActionError = "";
        // Esconde o painel: kdialog é coberto pelo layer-shell senão (mesmo caso do pickFolder).
        shellRoot.open = false;
        scanPickProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", "x", "pick-folder", ""];
        scanPickProc.running = true;
    }

    // Objetos plain do JS não notificam mudança de propriedade interna — pra marcar/desmarcar um
    // candidato é preciso reatribuir o array inteiro (reatribuição dispara o binding).
    function scanToggle(cwd): void {
        shellRoot.scanCandidates = shellRoot.scanCandidates.map(c => c.cwd === cwd ? Object.assign({}, c, {
                checked: !c.checked
            }) : c);
    }

    // Cadastra os candidatos marcados (e ainda não cadastrados), um de cada vez via projAdd —
    // o restante fica em scanQueue e é disparado pelo onStreamFinished do projActionProc.
    function scanAddChecked(): void {
        const pending = shellRoot.scanCandidates.filter(c => c.checked && !c.already);
        shellRoot.scanCandidates = [];
        if (pending.length === 0)
            return;
        shellRoot.scanQueue = pending.slice(1);
        shellRoot.projAdd(pending[0].name, pending[0].cwd, pending[0].command, "");
    }

    onOpenChanged: if (!open)
        menuSession = null;

    function toggle(): void {
        shellRoot.open = !shellRoot.open;
    }

    // Local: cp-panel-open foca a janela existente ou abre uma nova (attach cru duplicaria o
    // cliente tmux e encolheria a sessão). Remota: web UI do servidor dono — só o cp-send fala
    // com tmux remoto, abrir a UI é o equivalente útil no desktop.
    function activate(session: var): void {
        if (session.local) {
            Quickshell.execDetached([Quickshell.env("HOME") + "/.local/bin/cp-panel-open", session.name]);
        } else if (session.web_url) {
            Quickshell.execDetached(["xdg-open", session.web_url]);
        } else {
            return;   // peer sem web_url utilizável: não fecha o painel fingindo que abriu algo
        }
        shellRoot.open = false;
    }

    IpcHandler {
        target: "panel"

        function toggle(): void {
            shellRoot.toggle();
        }
        function open(): void {
            shellRoot.open = true;
        }
        function close(): void {
            shellRoot.open = false;
        }
        function settings(): void {
            // Ajustes virou ABA: o clique direito no tray cai direto nela, painel aberto.
            shellRoot.tab = "ajustes";
            shellRoot.open = true;
        }
        // Mesmo efeito do switch de servidor na aba de ajustes; existe pra script/teste
        // alcançar o toggle sem depender de clique real.
        function peer(id: string, state: string): void {
            shellRoot.setPeer(id, state === "on");
        }
        // Mesmo efeito do botão direito na linha; existe pra script/teste alcançar o menu.
        // Menu é de SESSÃO — força a aba certa antes de abrir.
        function menu(address: string): void {
            shellRoot.tab = "sessoes";
            const s = Sessions.sessions.find(x => x.address === address);
            shellRoot.menuSession = (s && shellRoot.menuSession?.address !== address) ? s : null;
            shellRoot.menuX = 40;
            shellRoot.menuY = 90;
        }
    }

    GlobalShortcut {
        appid: "claude-pocket"
        name: "toggle"
        onPressed: shellRoot.toggle()
    }

    // Toggle de servidor: liga/desliga um peer na varredura, gravando o peers.json pelo
    // cp-panel-action. Peer offline custa o timeout (4s) em TODO poll e, como a coleta espera o
    // servidor mais lento, é o painel inteiro que fica lento por causa de uma máquina desligada.
    function setPeer(id: string, on: bool): void {
        if (peerProc.running)
            return;
        shellRoot.peerResult = "";
        peerProc.command = [Quickshell.env("HOME") + "/.local/bin/cp-panel-action", id, "peer", on ? "on" : "off"];
        peerProc.running = true;
    }

    Process {
        id: peerProc
        stdout: StdioCollector {
            onStreamFinished: {
                let r = {};
                try {
                    r = JSON.parse(text);
                } catch (e) {
                    r = {
                        ok: false,
                        message: "resposta inválida do cp-panel-action"
                    };
                }
                // Falha VIRA texto na UI: sem isto, um peers.json somente-leitura deixava o
                // switch voltar sozinho no próximo poll, sem dizer que a gravação não aconteceu —
                // o usuário culparia o toggle "que não pega".
                shellRoot.peerResult = r.ok ? "" : (r.message ?? "falhou");
                Sessions.refresh();
            }
        }
    }

    PanelWindow {
        id: win

        visible: shellRoot.open
        color: "transparent"
        exclusiveZone: 0
        implicitWidth: 460
        // Cresce com o conteúdo até o teto; passando disso, as ListView rolam por dentro.
        implicitHeight: Math.min(720, content.implicitHeight + 24)
        WlrLayershell.namespace: "quickshell:claudePocket"
        WlrLayershell.layer: WlrLayer.Overlay
        WlrLayershell.keyboardFocus: shellRoot.open ? WlrKeyboardFocus.OnDemand : WlrKeyboardFocus.None

        anchors {
            top: true
            right: true
        }
        margins {
            top: 8
            right: 8
        }

        // Fecha ao clicar fora, do jeito do compositor (mesmo mecanismo que os painéis do rice
        // usam). Preferido ao truque de janela em tela cheia capturando clique: aquela versão
        // bloqueava o clique em TUDO enquanto aberta, inclusive na barra.
        HyprlandFocusGrab {
            active: shellRoot.open
            windows: [win]
            onCleared: shellRoot.open = false
        }

        onVisibleChanged: {
            // Poll rápido só enquanto visível (fechado, 10s basta pra notificação).
            Sessions.fast = visible;
            // A janela não é destruída ao fechar, então a ListView guardaria a rolagem anterior
            // e reabriria no meio — escondendo justo o topo, que é onde ficam as que esperam.
            if (visible)
                list.positionViewAtBeginning();
        }

        Item {
            id: windowRoot
            anchors.fill: parent
            focus: shellRoot.open
            Keys.onPressed: event => {
                // Esc fecha primeiro o menu de contexto, depois o painel — mesma escada de
                // qualquer menu: uma tecla não deve descartar dois níveis de uma vez.
                if (event.key !== Qt.Key_Escape)
                    return;
                if (shellRoot.menuSession)
                    shellRoot.menuSession = null;
                else
                    shellRoot.open = false;
            }

            Rectangle {
                id: content
                anchors.fill: parent
                anchors.margins: 12
                implicitHeight: layout.implicitHeight + 32
                radius: 22
                // Opacidade ajustável na aba de ajustes (persistida). Default acima do
                // ignore_alpha 0.79 que o rice aplica em "quickshell:.*": abaixo disso o
                // compositor pula o blur e sobra vidro sujo — a aba de ajustes avisa.
                color: Qt.rgba(0.094, 0.102, 0.122, PanelConfig.options.opacity)
                border.width: 1
                border.color: "#26ffffff"

                RowLayout {
                    id: layout
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 14

                    // Trilho de abas: ícones com badge. Coluna própria pra novas abas só
                    // custarem uma entrada no model.
                    ColumnLayout {
                        Layout.fillHeight: true
                        Layout.alignment: Qt.AlignTop
                        spacing: 6

                        Repeater {
                            model: [
                                {key: "sessoes", icon: "terminal"},
                                {key: "projetos", icon: "rocket_launch"},
                                {key: "ajustes", icon: "settings"}
                            ]

                            Rectangle {
                                id: tabBtn

                                required property var modelData
                                readonly property bool active: shellRoot.tab === tabBtn.modelData.key
                                // Badge: sessões esperando / projetos vivos. Vermelho = pede
                                // ação (esperando você, ou projeto que falhou).
                                readonly property int badge: tabBtn.modelData.key === "sessoes" ? Sessions.awaitingCount : tabBtn.modelData.key === "projetos" ? Sessions.projects.filter(p => p.state !== "stopped").length : 0
                                readonly property bool alert: tabBtn.modelData.key === "sessoes" ? Sessions.awaitingCount > 0 : tabBtn.modelData.key === "projetos" ? Sessions.projects.some(p => p.state === "failed") : false

                                implicitWidth: 40
                                implicitHeight: 40
                                radius: 12
                                color: tabBtn.active ? "#33ffffff" : tabMouse.containsMouse ? "#1fffffff" : "transparent"

                                Behavior on color {
                                    ColorAnimation {
                                        duration: 120
                                    }
                                }

                                Text {
                                    anchors.centerIn: parent
                                    text: tabBtn.modelData.icon
                                    font.family: "Material Symbols Rounded"
                                    font.pixelSize: 19
                                    color: tabBtn.active ? "#e3e2e6" : "#8e9099"
                                    renderType: Text.NativeRendering
                                }

                                Rectangle {
                                    visible: tabBtn.badge > 0
                                    anchors.top: parent.top
                                    anchors.right: parent.right
                                    anchors.topMargin: 1
                                    anchors.rightMargin: 1
                                    width: 15
                                    height: 15
                                    radius: 7.5
                                    color: tabBtn.alert ? "#f2b8b5" : "#8fce9b"

                                    Text {
                                        anchors.centerIn: parent
                                        text: tabBtn.badge
                                        color: "#1b1d24"
                                        font.pixelSize: 9
                                        font.weight: Font.DemiBold
                                    }
                                }

                                MouseArea {
                                    id: tabMouse
                                    anchors.fill: parent
                                    hoverEnabled: true
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: shellRoot.tab = tabBtn.modelData.key
                                }
                            }
                        }

                        Item {
                            Layout.fillHeight: true
                        }
                    }

                    // Página da aba ativa. Itens invisíveis saem do layout (comportamento
                    // padrão), então a altura da janela acompanha a aba.
                    ColumnLayout {
                        id: page
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        spacing: 12

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 8

                            Text {
                                text: "Claude Pocket"
                                color: "#e3e2e6"
                                font.pixelSize: 18
                                font.weight: Font.DemiBold
                                Layout.fillWidth: true
                            }

                            // Resumo numérico: o mesmo dado do badge, pra quem já está com o painel aberto.
                            Text {
                                visible: Sessions.awaitingCount > 0
                                text: `${Sessions.awaitingCount} esperando`
                                color: "#f2b8b5"
                                font.pixelSize: 12
                                font.weight: Font.DemiBold
                            }

                            Text {
                                visible: Sessions.workingCount > 0
                                text: `${Sessions.workingCount} rodando`
                                color: "#f9c784"
                                font.pixelSize: 12
                            }
                        }

                        // Erro de servidor aparece SEMPRE que existe, em qualquer aba: sem isto,
                        // um peer fora do ar sumiria silenciosamente da lista e pareceria "sem
                        // sessões".
                        Repeater {
                            model: Sessions.errors

                            Text {
                                required property string modelData
                                Layout.fillWidth: true
                                text: "⚠ " + modelData
                                color: "#f2b8b5"
                                font.pixelSize: 10
                                wrapMode: Text.Wrap
                            }
                        }

                        // ---- aba Sessões ------------------------------------------------------

                        Text {
                            visible: shellRoot.tab === "sessoes" && Sessions.everLoaded && Sessions.sessions.length === 0
                            Layout.fillWidth: true
                            text: "Nenhuma sessão viva."
                            color: "#a0a0a8"
                            font.pixelSize: 12
                        }

                        // ListView (não Repeater em Column): com 13+ sessões nos 3 servidores a lista
                        // passa da altura da tela — sem rolagem as de baixo ficavam inalcançáveis.
                        ListView {
                            id: list
                            visible: shellRoot.tab === "sessoes"
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            Layout.preferredHeight: Math.min(contentHeight, 480)
                            clip: true
                            spacing: 6
                            model: Sessions.sessions
                            boundsBehavior: Flickable.StopAtBounds
                            ScrollBar.vertical: ScrollBar {
                                policy: ScrollBar.AsNeeded
                            }

                            // Seções: bloco deste PC (attacha no terminal) separado de cada servidor
                            // remoto (só abre no navegador) — ação diferente, lista diferente.
                            section.property: "group"
                            section.delegate: Item {
                                required property string section
                                width: ListView.view.width
                                implicitHeight: 26

                                RowLayout {
                                    anchors.fill: parent
                                    anchors.topMargin: 8
                                    spacing: 6

                                    Text {
                                        text: parent.parent.section === "local" ? "computer" : "dns"
                                        font.family: "Material Symbols Rounded"
                                        font.pixelSize: 13
                                        color: "#8e9099"
                                    }

                                    Text {
                                        text: parent.parent.section === "local" ? "Este computador" : parent.parent.section
                                        color: "#8e9099"
                                        font.pixelSize: 10
                                        font.weight: Font.DemiBold
                                        font.capitalization: Font.AllUppercase
                                    }

                                    Rectangle {
                                        Layout.fillWidth: true
                                        implicitHeight: 1
                                        color: "#1affffff"
                                    }
                                }
                            }

                            delegate: SessionRow {
                                required property var modelData
                                session: modelData
                                width: ListView.view.width
                                onActivated: shellRoot.activate(modelData)
                                onMenuRequested: (sx, sy) => {
                                    shellRoot.menuSession = modelData;
                                    shellRoot.menuX = sx;
                                    shellRoot.menuY = sy;
                                }
                            }
                        }

                        // Legenda: ícone e rótulo são Text separados de propósito — o glifo só sai
                        // com font.family Material Symbols, então não cabe na mesma string do texto.
                        RowLayout {
                            visible: shellRoot.tab === "sessoes"
                            Layout.fillWidth: true
                            spacing: 4

                            Repeater {
                                model: [
                                    {icon: "terminal", label: "focar"},
                                    {icon: "add", label: "abrir"},
                                    {icon: "language", label: "web"}
                                ]

                                RowLayout {
                                    required property var modelData
                                    spacing: 3

                                    Text {
                                        text: parent.modelData.icon
                                        font.family: "Material Symbols Rounded"
                                        font.pixelSize: 13
                                        color: "#6e7079"
                                    }

                                    Text {
                                        text: parent.modelData.label
                                        font.pixelSize: 10
                                        color: "#6e7079"
                                        rightPadding: 6
                                    }
                                }
                            }

                            Item {
                                Layout.fillWidth: true
                            }

                            Text {
                                text: "Esc fecha"
                                font.pixelSize: 10
                                color: "#6e7079"
                            }
                        }

                        // ---- aba Projetos -----------------------------------------------------
                        // Hierarquia do control center: card completo só pro que está VIVO (ou
                        // falhou — pede leitura de log); os parados viram grade de tiles de um
                        // toque. 15 linhas de "parado" repetido não merecem lista cheia.

                        Text {
                            visible: shellRoot.tab === "projetos" && Sessions.projects.length === 0
                            Layout.fillWidth: true
                            text: "Nenhum projeto no backend/projects.json."
                            color: "#a0a0a8"
                            font.pixelSize: 12
                        }

                        Flickable {
                            id: projFlick
                            visible: shellRoot.tab === "projetos" && Sessions.projects.length > 0
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            Layout.preferredHeight: Math.min(contentHeight, 560)
                            contentHeight: projCol.implicitHeight
                            clip: true
                            boundsBehavior: Flickable.StopAtBounds
                            ScrollBar.vertical: ScrollBar {
                                policy: ScrollBar.AsNeeded
                            }

                            ColumnLayout {
                                id: projCol
                                width: projFlick.width
                                spacing: 6

                                RowLayout {
                                    visible: shellRoot.activeProjects.length > 0
                                    Layout.fillWidth: true
                                    spacing: 6

                                    Text {
                                        text: "Em execução"
                                        color: "#8e9099"
                                        font.pixelSize: 10
                                        font.weight: Font.DemiBold
                                        font.capitalization: Font.AllUppercase
                                    }

                                    Rectangle {
                                        Layout.fillWidth: true
                                        implicitHeight: 1
                                        color: "#1affffff"
                                    }
                                }

                                Text {
                                    visible: shellRoot.activeProjects.length === 0
                                    Layout.fillWidth: true
                                    text: "Nada rodando — toque num projeto pra subir."
                                    color: "#a0a0a8"
                                    font.pixelSize: 11
                                }

                                Repeater {
                                    model: shellRoot.activeProjects

                                    ProjectRow {
                                        required property var modelData
                                        project: modelData
                                        Layout.fillWidth: true
                                        onChanged: Sessions.refresh()
                                    }
                                }

                                RowLayout {
                                    visible: shellRoot.stoppedProjects.length > 0
                                    Layout.fillWidth: true
                                    Layout.topMargin: 6
                                    spacing: 6

                                    Text {
                                        text: "Parados"
                                        color: "#8e9099"
                                        font.pixelSize: 10
                                        font.weight: Font.DemiBold
                                        font.capitalization: Font.AllUppercase
                                    }

                                    Rectangle {
                                        Layout.fillWidth: true
                                        implicitHeight: 1
                                        color: "#1affffff"
                                    }
                                }

                                // Grade 2 colunas de tiles (padrão dos toggles do rice): um
                                // toque = play. Detalhe/stop/log só existem no card, e o
                                // projeto sobe pra lá sozinho assim que sai de "parado".
                                GridLayout {
                                    Layout.fillWidth: true
                                    columns: 2
                                    columnSpacing: 6
                                    rowSpacing: 6

                                    Repeater {
                                        model: shellRoot.stoppedProjects

                                        Rectangle {
                                            id: tile

                                            required property var modelData
                                            readonly property bool busy: shellRoot.startingProject === tile.modelData.name

                                            Layout.fillWidth: true
                                            implicitHeight: 40
                                            radius: 12
                                            color: tile.busy ? "#33ffffff" : tileMouse.containsMouse ? "#28ffffff" : "#14ffffff"

                                            Behavior on color {
                                                ColorAnimation {
                                                    duration: 120
                                                }
                                            }

                                            RowLayout {
                                                anchors.fill: parent
                                                anchors.leftMargin: 12
                                                anchors.rightMargin: 10
                                                spacing: 8

                                                Text {
                                                    Layout.fillWidth: true
                                                    text: tile.modelData.name
                                                    color: "#c8c6cf"
                                                    font.pixelSize: 11
                                                    elide: Text.ElideMiddle
                                                }

                                                Text {
                                                    text: tile.busy ? "hourglass_empty" : "play_arrow"
                                                    color: tile.busy ? "#f9c784" : "#8fce9b"
                                                    font.family: "Material Symbols Rounded"
                                                    font.pixelSize: 16
                                                    renderType: Text.NativeRendering
                                                }
                                            }

                                            MouseArea {
                                                id: tileMouse
                                                anchors.fill: parent
                                                hoverEnabled: true
                                                cursorShape: Qt.PointingHandCursor
                                                enabled: !projStartProc.running
                                                onClicked: shellRoot.startProject(tile.modelData.name)
                                            }
                                        }
                                    }
                                }

                                Text {
                                    visible: shellRoot.projectError !== ""
                                    Layout.fillWidth: true
                                    text: shellRoot.projectError
                                    color: "#f28b82"
                                    font.pixelSize: 9
                                    wrapMode: Text.Wrap
                                }
                            }
                        }

                        // ---- aba Ajustes ------------------------------------------------------
                        // Flickable espelhando o padrão da aba Projetos (projFlick/projCol):
                        // com 13+ projetos cadastrados + o form, o conteúdo passa da altura do
                        // painel — sem rolagem o cartão "Novo projeto" ficava inalcançável.

                        Flickable {
                            id: ajustesFlick
                            visible: shellRoot.tab === "ajustes"
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            Layout.preferredHeight: Math.min(contentHeight, 560)
                            contentHeight: ajustesCol.implicitHeight
                            clip: true
                            boundsBehavior: Flickable.StopAtBounds
                            ScrollBar.vertical: ScrollBar {
                                policy: ScrollBar.AsNeeded
                            }

                            ColumnLayout {
                                id: ajustesCol
                                width: ajustesFlick.width
                                spacing: 2

                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 8

                                    Text {
                                        text: "Transparência"
                                        color: "#c8c6cf"
                                        font.pixelSize: 11
                                    }

                                    Slider {
                                        id: opacitySlider
                                        Layout.fillWidth: true
                                        from: 0.5
                                        to: 1.0
                                        value: PanelConfig.options.opacity
                                        onMoved: PanelConfig.options.opacity = value
                                    }

                                    Text {
                                        text: Math.round(PanelConfig.options.opacity * 100) + "%"
                                        color: "#c8c6cf"
                                        font.pixelSize: 11
                                        // Largura fixa: sem isto o slider pulava a cada dígito trocado.
                                        Layout.preferredWidth: 32
                                        horizontalAlignment: Text.AlignRight
                                    }
                                }

                                Text {
                                    Layout.fillWidth: true
                                    // O corte não é escolha minha: é o ignore_alpha do rice. Dizer o
                                    // porquê evita o usuário achar que o blur "bugou" ao baixar demais.
                                    text: PanelConfig.options.opacity < PanelConfig.blurCutoff ? "sem desfoque abaixo de 79% (limite do rice)" : "com desfoque do compositor"
                                    color: PanelConfig.options.opacity < PanelConfig.blurCutoff ? "#f9c784" : "#6e7079"
                                    font.pixelSize: 9
                                }

                                // Servidores da malha. Só aparece com peer configurado — máquina
                                // sozinha não tem por que ganhar uma seção vazia.
                                Text {
                                    visible: Sessions.servers.length > 0
                                    Layout.topMargin: 8
                                    text: "Servidores"
                                    color: "#c8c6cf"
                                    font.pixelSize: 11
                                }

                                // Tiles-pílula (padrão Internet/Bluetooth do rice): aceso = na
                                // varredura, apagado = fora. Toque alterna — mais direto que uma
                                // linha com switch pra um estado binário.
                                GridLayout {
                                    Layout.fillWidth: true
                                    Layout.topMargin: 4
                                    columns: 2
                                    columnSpacing: 6
                                    rowSpacing: 6

                                    Repeater {
                                        model: Sessions.servers

                                        Rectangle {
                                            id: srvTile

                                            required property var modelData
                                            readonly property bool on: srvTile.modelData.enabled

                                            Layout.fillWidth: true
                                            implicitHeight: 46
                                            radius: 14
                                            color: srvTile.on ? "#e3e2e6" : srvMouse.containsMouse ? "#28ffffff" : "#14ffffff"

                                            Behavior on color {
                                                ColorAnimation {
                                                    duration: 120
                                                }
                                            }

                                            RowLayout {
                                                anchors.fill: parent
                                                anchors.leftMargin: 12
                                                anchors.rightMargin: 12
                                                spacing: 8

                                                Text {
                                                    text: "dns"
                                                    font.family: "Material Symbols Rounded"
                                                    font.pixelSize: 16
                                                    color: srvTile.on ? "#1b1d24" : "#8e9099"
                                                    renderType: Text.NativeRendering
                                                }

                                                ColumnLayout {
                                                    Layout.fillWidth: true
                                                    spacing: 0

                                                    Text {
                                                        Layout.fillWidth: true
                                                        text: srvTile.modelData.id
                                                        color: srvTile.on ? "#1b1d24" : "#c8c6cf"
                                                        font.pixelSize: 11
                                                        font.weight: Font.DemiBold
                                                        elide: Text.ElideRight
                                                    }

                                                    Text {
                                                        Layout.fillWidth: true
                                                        // Estado REAL, não o do tile: peer LIGADO que
                                                        // não responde precisa aparecer como problema.
                                                        // Tile aceso não é garantia de máquina viva.
                                                        text: !srvTile.on ? "fora da varredura" : (srvTile.modelData.ok ? "respondendo" : "sem resposta")
                                                        color: !srvTile.on ? "#6e7079" : (srvTile.modelData.ok ? "#3d6b46" : "#8a5a00")
                                                        font.pixelSize: 9
                                                        elide: Text.ElideRight
                                                    }
                                                }
                                            }

                                            MouseArea {
                                                id: srvMouse
                                                anchors.fill: parent
                                                hoverEnabled: true
                                                cursorShape: Qt.PointingHandCursor
                                                enabled: !peerProc.running
                                                onClicked: shellRoot.setPeer(srvTile.modelData.id, !srvTile.on)
                                            }
                                        }
                                    }
                                }

                                Text {
                                    visible: shellRoot.peerResult !== ""
                                    Layout.fillWidth: true
                                    text: shellRoot.peerResult
                                    color: "#f28b82"
                                    font.pixelSize: 9
                                    wrapMode: Text.Wrap
                                }

                                // ---- Projetos do launcher (cadastro) --------------------------
                                Text {
                                    Layout.topMargin: 10
                                    text: "Projetos"
                                    color: "#c8c6cf"
                                    font.pixelSize: 11
                                }

                                // Lista dos cadastrados: nome + pasta, com botão remover.
                                Repeater {
                                    model: Sessions.projects
                                    RowLayout {
                                        required property var modelData
                                        Layout.fillWidth: true
                                        spacing: 6
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 0
                                            Text {
                                                text: parent.parent.modelData.name
                                                color: "#e3e2e6"
                                                font.pixelSize: 11
                                            }
                                            Text {
                                                Layout.fillWidth: true
                                                text: parent.parent.modelData.cwd
                                                color: "#6e7079"
                                                font.pixelSize: 9
                                                elide: Text.ElideMiddle
                                            }
                                        }
                                        // remover
                                        Text {
                                            text: "delete"
                                            font.family: "Material Symbols Rounded"
                                            font.pixelSize: 16
                                            color: delMouse.containsMouse ? "#f28b82" : "#8e9099"
                                            MouseArea {
                                                id: delMouse
                                                anchors.fill: parent
                                                hoverEnabled: true
                                                cursorShape: Qt.PointingHandCursor
                                                onClicked: shellRoot.projDel(parent.parent.modelData.name)
                                            }
                                        }
                                    }
                                }

                                // ---- Varrer uma pasta pai (cadastro em lote) -------------------
                                Text {
                                    Layout.topMargin: 10
                                    text: "Varrer uma pasta"
                                    color: "#c8c6cf"
                                    font.pixelSize: 11
                                }

                                Button {
                                    text: "📁 escolher pasta e varrer"
                                    onClicked: shellRoot.scanStart()
                                }

                                // Cada subpasta encontrada: checkbox de seleção (já cadastrado vem
                                // desmarcado e desabilitado), nome, comando detectado em cinza.
                                Repeater {
                                    model: shellRoot.scanCandidates
                                    RowLayout {
                                        id: scanRow

                                        required property var modelData
                                        Layout.fillWidth: true
                                        spacing: 6

                                        CheckBox {
                                            checked: scanRow.modelData.checked
                                            enabled: !scanRow.modelData.already
                                            onToggled: shellRoot.scanToggle(scanRow.modelData.cwd)
                                        }

                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 0
                                            Text {
                                                Layout.fillWidth: true
                                                text: scanRow.modelData.name + (scanRow.modelData.already ? "  · já cadastrado" : "")
                                                color: scanRow.modelData.already ? "#6e7079" : "#e3e2e6"
                                                font.pixelSize: 11
                                                elide: Text.ElideRight
                                            }
                                            Text {
                                                Layout.fillWidth: true
                                                text: scanRow.modelData.command !== "" ? scanRow.modelData.command : "sem comando"
                                                color: "#6e7079"
                                                font.pixelSize: 9
                                                elide: Text.ElideRight
                                            }
                                        }
                                    }
                                }

                                Button {
                                    visible: shellRoot.scanCandidates.length > 0
                                    text: "cadastrar marcados (" + shellRoot.scanCandidates.filter(c => c.checked && !c.already).length + ")"
                                    enabled: shellRoot.scanCandidates.some(c => c.checked && !c.already)
                                    onClicked: shellRoot.scanAddChecked()
                                }

                                // Título do cartão de cadastro.
                                Text {
                                    Layout.topMargin: 8
                                    text: "Novo projeto"
                                    color: "#c8c6cf"
                                    font.pixelSize: 11
                                }

                                // Form dentro de um cartão sutil — separa visualmente do resto da
                                // aba em vez de campos soltos empilhados.
                                Rectangle {
                                    Layout.fillWidth: true
                                    Layout.topMargin: 4
                                    radius: 12
                                    color: "#14ffffff"
                                    implicitHeight: formCol.implicitHeight + 20

                                    ColumnLayout {
                                        id: formCol
                                        anchors.fill: parent
                                        anchors.margins: 10
                                        spacing: 8

                                        TextField {
                                            id: fName
                                            Layout.fillWidth: true
                                            placeholderText: "nome"
                                            color: "#e3e2e6"
                                            placeholderTextColor: "#6e7079"
                                            background: Rectangle {
                                                radius: 8
                                                color: "#1e1e26"
                                                border.color: "#33ffffff"
                                                border.width: 1
                                            }
                                        }

                                        // Pasta do projeto: caixa de diálogo NATIVA do sistema
                                        // (kdialog/zenity via pick-folder) no lugar do navegador
                                        // embutido — o campo só mostra o path escolhido.
                                        RowLayout {
                                            Layout.fillWidth: true
                                            spacing: 6
                                            TextField {
                                                id: fCwd
                                                Layout.fillWidth: true
                                                readOnly: true
                                                placeholderText: "pasta do projeto"
                                                color: "#e3e2e6"
                                                placeholderTextColor: "#6e7079"
                                                background: Rectangle {
                                                    radius: 8
                                                    color: "#1e1e26"
                                                    border.color: "#33ffffff"
                                                    border.width: 1
                                                }
                                            }
                                            Button {
                                                text: "📁 escolher pasta"
                                                onClicked: shellRoot.pickFolder()
                                            }
                                        }

                                        TextField {
                                            id: fCommand
                                            Layout.fillWidth: true
                                            placeholderText: "comando (ex: pnpm dev)"
                                            color: "#e3e2e6"
                                            placeholderTextColor: "#6e7079"
                                            background: Rectangle {
                                                radius: 8
                                                color: "#1e1e26"
                                                border.color: "#33ffffff"
                                                border.width: 1
                                            }
                                        }
                                        TextField {
                                            id: fPort
                                            Layout.fillWidth: true
                                            placeholderText: "porta (opcional)"
                                            color: "#e3e2e6"
                                            placeholderTextColor: "#6e7079"
                                            background: Rectangle {
                                                radius: 8
                                                color: "#1e1e26"
                                                border.color: "#33ffffff"
                                                border.width: 1
                                            }
                                        }
                                        Button {
                                            text: "＋ adicionar projeto"
                                            enabled: fName.text.trim() !== "" && fCwd.text.trim() !== "" && fCommand.text.trim() !== ""
                                            onClicked: {
                                                shellRoot.projAdd(fName.text.trim(), fCwd.text.trim(), fCommand.text.trim(), fPort.text);
                                                fName.text = "";
                                                fCwd.text = "";
                                                fCommand.text = "";
                                                fPort.text = "";
                                            }
                                        }
                                    }
                                }

                                // Importar de outra máquina: um botão por peer -> lista candidatos.
                                RowLayout {
                                    visible: Sessions.servers.length > 0
                                    Layout.topMargin: 8
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Text {
                                        text: "Importar de:"
                                        color: "#c8c6cf"
                                        font.pixelSize: 11
                                    }
                                    Repeater {
                                        model: Sessions.servers
                                        Button {
                                            required property var modelData
                                            text: modelData.id
                                            onClicked: shellRoot.importFrom(modelData.id)
                                        }
                                    }
                                }

                                // Candidatos casados: um toque cadastra (só os matched têm pasta local).
                                Repeater {
                                    model: shellRoot.importCandidates
                                    RowLayout {
                                        required property var modelData
                                        Layout.fillWidth: true
                                        spacing: 6
                                        Text {
                                            Layout.fillWidth: true
                                            text: (modelData.matched ? "✓ " : "✗ ") + modelData.name
                                            color: modelData.matched ? "#a5d6a7" : "#6e7079"
                                            font.pixelSize: 10
                                            elide: Text.ElideRight
                                        }
                                        Button {
                                            text: "adicionar"
                                            enabled: modelData.matched === true
                                            onClicked: shellRoot.projAdd(modelData.name, modelData.cwd, modelData.command, modelData.port)
                                        }
                                    }
                                }

                                Text {
                                    visible: shellRoot.projActionError !== ""
                                    Layout.fillWidth: true
                                    text: shellRoot.projActionError
                                    color: "#f28b82"
                                    font.pixelSize: 9
                                    wrapMode: Text.Wrap
                                }
                            }
                        }
                    }
                }
            }

            // Camada do menu de contexto: irmã do card e DEPOIS dele, então fica por cima de
            // tudo sem z-index manual. Cobre a janela inteira pra capturar o clique de dispensa.
            Item {
                id: menuLayer
                anchors.fill: parent
                visible: shellRoot.menuSession !== null

                MouseArea {
                    anchors.fill: parent
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    onClicked: shellRoot.menuSession = null
                }

                Loader {
                    active: shellRoot.menuSession !== null

                    // Preso dentro da janela: aberto perto da borda direita/inferior, o menu
                    // sairia da tela — o layer-shell recorta e ele viraria um toco inclicável.
                    x: Math.max(8, Math.min(shellRoot.menuX, menuLayer.width - width - 8))
                    y: Math.max(8, Math.min(shellRoot.menuY, menuLayer.height - height - 8))

                    sourceComponent: SessionMenu {
                        session: shellRoot.menuSession
                        onClosed: shellRoot.menuSession = null
                        onChanged: Sessions.refresh()
                    }
                }
            }
        }
    }
}
