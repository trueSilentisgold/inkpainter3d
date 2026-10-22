---
clickup: 1amq5eu1z3w
url: https://app.clickup.com/t/1amq5eu1z3w
status: in-arbeit
prioritaet: hoch
liste: Features
---

# Bugs melden und Crash-Reports senden

Bugs lassen sich bequem melden, und nach einem Absturz kann ein Crash-Report gesendet werden. Alles läuft über GitHub Issues.

- Help → „Report a bug…“: kurzer Dialog, öffnet ein vorausgefülltes GitHub-Issue (Formular-Vorlage)
- Panic-Hook schreibt einen Crash-Report in den Datenordner; beim nächsten Start fragt die App, ob er gemeldet werden soll. Der Bericht wird vorher gezeigt und nur auf Klick gesendet.
- Inhalt: Version, System, GPU (Name, Treiber, Backend), Fehlermeldung und Backtrace, die letzten rund 200 Log-Zeilen, Projekt-Eckdaten (Anzahl Texture Sets, Auflösungen, Layer-Anzahl, ohne Namen, Pfade oder Bilder)
- GitHub Issue Forms für Bug und Crash

Grenze: Native Abstürze (Segfault, Treiber-Abbruch) fängt der Panic-Hook nicht.

## Notizen

- Ziel ist GitHub Issues (Entscheidung des Owners, 2026-10-22). Eine Discord-Weiterleitung ohne Account bräuchte einen eigenen Server, weil eine Webhook-URL in der App jeder auslesen könnte.
- GitHub nimmt URLs nur bis etwa 8 KB. Zu lange Felder kürzt `issue_url`: beim Log fallen die ältesten Zeilen weg, beim Backtrace die letzten. Der volle Bericht liegt in der Zwischenablage und in der Crash-Datei.
- eframe hatte das Feature `links` nicht aktiv. Ohne das öffnen Links nichts, auch der INKRABBIT-Link im About-Dialog nicht. Das ist jetzt an.
- Das Windows-Release legt die `.pdb` neben die `.exe`, damit Backtraces Funktionsnamen zeigen.
- Getestet unter Windows mit `INKPAINTER3D_CRASH_TEST=1`: Der Bericht wird geschrieben, beim nächsten Start kommt der Dialog, „Don't report“ verschiebt den Bericht nach `crashes/seen`. Der Bug-Dialog zeigt Version, GPU mit Treiber und die Projekt-Eckdaten.
- Test des Owners (Issue #12): Das Issue kam ohne Text an, weil die App Felder eines Formulars füllte, das auf `main` noch fehlte. Jetzt steht der Bericht als Markdown im Body (`body=`), das geht ohne Formulare. Die Formulare bleiben für Issues, die direkt auf GitHub eröffnet werden.
