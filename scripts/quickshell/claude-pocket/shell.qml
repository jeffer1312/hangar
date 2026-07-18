//@ pragma UseQApplication
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Hyprland

// Painel "control center" do claude-pocket: lista as sessões dos 3 servidores, clique abre o
// terminal já attachado (local) ou a web UI (remota). Instância SEPARADA do quickshell
// (qs -c claude-pocket) de propósito: não toca em nenhum arquivo do rice, então update do
// dots-hyprland não apaga isto e um bug daqui não derruba a barra.
ShellRoot {
    id: shellRoot

    property bool open: false
    // Sessão com menu de contexto aberto (null = nenhum) e o ponto onde ele abre. Um só por vez.
    property var menuSession: null
    property real menuX: 0
    property real menuY: 0

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
            win.settingsOpen = !win.settingsOpen;
        }
        // Mesmo efeito do botão direito na linha; existe pra script/teste alcançar o menu.
        function menu(address: string): void {
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

    PanelWindow {
        id: win

        property bool settingsOpen: false

        // Abrir/fechar os ajustes redimensiona a lista; sem reposicionar, ela fica presa no
        // contentY antigo e mostra meia linha cortada no topo.
        onSettingsOpenChanged: list.positionViewAtBeginning()

        visible: shellRoot.open
        color: "transparent"
        exclusiveZone: 0
        implicitWidth: 460
        // Cresce com o conteúdo até o teto; passando disso, a ListView rola por dentro.
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
                // Opacidade ajustável no botão de config (persistida). Default acima do
                // ignore_alpha 0.79 que o rice aplica em "quickshell:.*": abaixo disso o
                // compositor pula o blur e sobra vidro sujo — a faixa de ajustes avisa.
                color: Qt.rgba(0.094, 0.102, 0.122, PanelConfig.options.opacity)
                border.width: 1
                border.color: "#26ffffff"

                ColumnLayout {
                    id: layout
                    anchors.fill: parent
                    anchors.margins: 16
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

                        // Engrenagem: abre a faixa de ajustes embutida (não outra janela — um
                        // painel de 1 opção não justifica segunda camada de foco/dismiss).
                        Text {
                            text: "settings"
                            font.family: "Material Symbols Rounded"
                            font.pixelSize: 18
                            color: settingsMouse.containsMouse || win.settingsOpen ? "#e3e2e6" : "#8e9099"

                            MouseArea {
                                id: settingsMouse
                                anchors.fill: parent
                                anchors.margins: -6
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: win.settingsOpen = !win.settingsOpen
                            }
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

                    // Faixa de ajustes: some por completo quando fechada (height 0) pra não
                    // roubar altura da lista, que é o conteúdo principal.
                    Rectangle {
                        Layout.fillWidth: true
                        Layout.preferredHeight: win.settingsOpen ? 64 : 0
                        clip: true
                        radius: 12
                        color: "#14ffffff"
                        visible: Layout.preferredHeight > 0

                        Behavior on Layout.preferredHeight {
                            NumberAnimation {
                                duration: 140
                                easing.type: Easing.OutCubic
                            }
                        }

                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: 10
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
                        }
                    }

                    // Erro de servidor aparece SEMPRE que existe: sem isto, um peer fora do ar
                    // sumiria silenciosamente da lista e pareceria "sem sessões".
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

                    Text {
                        visible: Sessions.everLoaded && Sessions.sessions.length === 0
                        Layout.fillWidth: true
                        text: "Nenhuma sessão viva."
                        color: "#a0a0a8"
                        font.pixelSize: 12
                    }

                    // ListView (não Repeater em Column): com 13+ sessões nos 3 servidores a lista
                    // passa da altura da tela — sem rolagem as de baixo ficavam inalcançáveis.
                    ListView {
                        id: list
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

                    // (menu de contexto flutua na camada acima — ver menuLayer)

                    // Legenda: ícone e rótulo são Text separados de propósito — o glifo só sai
                    // com font.family Material Symbols, então não cabe na mesma string do texto.
                    RowLayout {
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
