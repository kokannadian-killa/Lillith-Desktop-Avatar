// DemonOS PhantomCore SDDM greeter (seen only when autologin is off or after logout).
import QtQuick 2.15
import SddmComponents 2.0

Rectangle {
    id: root
    width: 1920; height: 1080
    color: "#0a0a0f"

    property int sessionIndex: sessionModel.lastIndex

    Connections {
        target: sddm
        function onLoginFailed() { pw.text = ""; status.text = "the core rejects you — try again"; status.color = "#ff3e6c" }
    }

    Image {
        anchors.fill: parent
        source: config.background
        fillMode: Image.PreserveAspectCrop
    }

    Rectangle {
        id: panel
        width: 460; height: 300
        anchors.left: parent.left; anchors.leftMargin: parent.width * 0.08
        anchors.verticalCenter: parent.verticalCenter; anchors.verticalCenterOffset: parent.height * 0.22
        color: "#cc0a0a0f"; border.color: "#b829f7"; border.width: 2; radius: 6

        Column {
            anchors.centerIn: parent; spacing: 14; width: parent.width - 60

            Text { text: "> IDENTIFY YOURSELF_"; color: "#00f0ff"; font.family: "JetBrains Mono"; font.pixelSize: 18 }

            TextBox {
                id: user; width: parent.width; height: 36
                text: userModel.lastUser
                color: "#14121c"; borderColor: "#ff3e6c"; focusColor: "#00f0ff"; hoverColor: "#b829f7"; textColor: "#e6e1f0"
                font.family: "JetBrains Mono"; font.pixelSize: 15
                KeyNavigation.tab: pw
            }
            PasswordBox {
                id: pw; width: parent.width; height: 36
                color: "#14121c"; borderColor: "#ff3e6c"; focusColor: "#00f0ff"; hoverColor: "#b829f7"; textColor: "#e6e1f0"
                font.pixelSize: 15
                focus: true
                Keys.onPressed: function(event) {
                    if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                        sddm.login(user.text, pw.text, root.sessionIndex); event.accepted = true
                    }
                }
                KeyNavigation.tab: login
            }
            Button {
                id: login; width: parent.width; height: 38
                text: "SUMMON"
                color: "#ff3e6c"; textColor: "#0a0a0f"; activeColor: "#00f0ff"; pressedColor: "#b829f7"
                font.family: "JetBrains Mono"; font.pixelSize: 16
                onClicked: sddm.login(user.text, pw.text, root.sessionIndex)
            }
            Text { id: status; text: "live user: demon (no password)"; color: "#9690aa"; font.family: "JetBrains Mono"; font.pixelSize: 12 }
        }
    }

    Text {
        anchors.right: parent.right; anchors.bottom: parent.bottom; anchors.margins: 24
        text: Qt.formatDateTime(new Date(), "yyyy-MM-dd  hh:mm"); color: "#00f0ff"
        font.family: "JetBrains Mono"; font.pixelSize: 14
    }
}
