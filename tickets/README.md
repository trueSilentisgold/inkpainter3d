# Tickets

Offene Aufgaben für inkpainter3d. **ClickUp ist die Quelle**
(Space „Inkrabbti“ → Ordner „inkpainter3d“, Listen Features, Bugs und Releases).
Hier im Repo liegt pro ClickUp-Aufgabe eine lokale Kopie, damit man beim Arbeiten
am Code alles beisammen hat und der Verlauf in Git steht.

`docs/FEATURES.md` bleibt der vollständige Feature-Katalog mit Prioritäten und
Phasenplan. Die offenen P0- und P1-Punkte daraus stehen als Aufgaben in der Liste
„Features“. Kommt eine Funktion dazu, wird sie in beiden abgehakt.

## Ablauf

1. **Neue Aufgabe:** zuerst in ClickUp anlegen, nicht hier. Beim nächsten
   Abgleich entsteht daraus ein lokales Ticket.
2. **Abgleich ClickUp → lokal** (z. B. „gleich die Tickets ab“ oder `/tickets`):
   Claude holt alle Aufgaben aus dem Ordner „inkpainter3d“ und legt für jede
   ohne lokale Datei ein Ticket an. Titel, Priorität, Fälligkeit und
   Beschreibung aus ClickUp überschreiben die lokale Kopie, die Notizen bleiben.
   In ClickUp erledigte oder gelöschte Aufgaben wandern nach `erledigt/`.
3. **Arbeit beginnt:** `status: in-arbeit` im Ticket, in ClickUp „in progress“.
4. **Fertig:** `status: erledigt`, `erledigt:` mit Datum und `commits:` füllen,
   die Datei nach `erledigt/` verschieben. In ClickUp auf „complete“ setzen und
   einen Kommentar mit Commit und Kurzfassung hinterlassen. „Fertig“ heißt:
   gebaut, geprüft und auf `main` gemergt. Was nur auf einem Branch liegt,
   bleibt `in-arbeit`.

## Dateiformat

Dateiname: `<clickup-id>-<titel-als-slug>.md`

```markdown
---
clickup: abc123                       # Task-ID in ClickUp
url: https://app.clickup.com/t/abc123
status: offen                         # offen | in-arbeit | erledigt
prioritaet: normal                    # dringend | hoch | normal | niedrig | keine
liste: Listenname                     # ClickUp-Liste
eltern: def456                        # optional: übergeordnete Aufgabe
faellig: 2026-10-25                   # optional
erledigt: 2026-10-30                  # erst beim Abschluss
commits: a1b2c3d, e4f5a6b             # erst beim Abschluss
---

# Titel

Beschreibung aus ClickUp.

## Notizen

Was beim Arbeiten auffällt: Entscheidungen, offene Fragen, Stolpersteine.
```

Erledigte Funktionen werden außerdem in `docs/FEATURES.md` und im README
(Statuszeile, „Available now“, Roadmap) eingetragen.
