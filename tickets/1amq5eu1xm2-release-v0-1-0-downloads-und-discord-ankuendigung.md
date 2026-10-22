---
clickup: 1amq5eu1xm2
url: https://app.clickup.com/t/1amq5eu1xm2
status: in-arbeit
prioritaet: hoch
liste: Releases
---

# Release v0.1.0: Downloads und Discord-Ankündigung

Erste fertig gebaute Version für Windows, Linux und macOS.

- Release-Workflow (`.github/workflows/release.yml`): ein Tag `v*` baut die Downloads und legt das GitHub Release an. PR #11.
- Discord-Ankündigung (`announce.yml`): Text und Update-Bild in `.github/announcements/v0.1.0/`, alles auf Englisch. Gepostet wird erst nach Freigabe des Owners.
- App-Icon aus `app-icon-large.svg`: PNGs, Fenster-Icon, eingebettet in die Windows-`.exe`, macOS-Icon.

Offen:
1. Owner gibt Bild und Text frei, dann Commit auf PR #11
2. Secret `DISCORD_WEBHOOK_URL` in GitHub anlegen
3. PR #11 mergen
4. Owner setzt den Tag `v0.1.0`
5. Release prüfen, Ankündigung nach Freigabe posten

## Notizen

