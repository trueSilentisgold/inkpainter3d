# Funktionsumfang

Dieses Dokument legt fest, **was inkpainter3d können muss**, damit es Substance 3D Painter im
Alltag ersetzt. Vorbild ist Painter in Version 12.x (Stand Oktober 2026).

## Leitlinien

1. **Gleiche Konzepte, gleiche Begriffe, gleiche Shortcuts wie Painter.** Wer Painter kann,
   soll sofort loslegen können, und Painter-Tutorials sollen sich übertragen lassen. Begriffe wie
   *Texture Set*, *Fill Layer*, *Smart Mask* oder *Anchor Point* bleiben deshalb englisch.
2. **Abweichen nur, wo es bewusst besser ist.** Solche Stellen sind mit **＋** markiert.
3. **Kein Adobe-Material.** Name, Logo, Icons, mitgelieferte Materialien und das
   `.sbsar`-Format werden nicht übernommen (siehe [Bewusst nicht übernommen](#bewusst-nicht-übernommen)).
   Funktionen und Bedienabläufe dagegen schon.
4. **Linux ist Hauptplattform, Windows gleichwertig.** Painter gibt es für Linux nur mit
   Enterprise-Lizenz (RHEL) oder als Steam-Version (Ubuntu 22.04), und Mesa-Treiber werden nicht
   unterstützt. inkpainter3d soll auf jeder Vulkan-fähigen GPU laufen, auch mit Mesa (AMD, Intel).
5. **Realistisches PBR hat Vorrang.** Stylized und Toon kommen auch, sind aber zweitrangig.
   Ziel-Engines sind **Blender, Unity und Godot**.

## Referenzprojekt

Ein echtes Painter-Projekt (Painter 12.1.5) dient als Abnahmetest: **Was dort gebaut wurde, muss
sich in inkpainter3d genauso bauen lassen.** Es enthält:

- 5 Texture Sets mit je 4096², die sich einen Shader teilen: Körper, Extras, Ohren,
  Alpha-Test-Karten (Fell, Schnurrhaare) und Alpha-Blend-Teile
- Ordner, zum Teil mit Blend Mode *Multiply*
- Fill Layer mit Masken; in einer Maske ein Blur-Filter mit Blend Mode *Replace*
- Leuchtende Linien über den Emissive-Kanal
- Kanäle Base Color, Roughness, Metallic, Normal, Height, Emissive, Opacity
- Arbeit mit 3D- und 2D-Ansicht nebeneinander (Layout F1)
- Materialien aus einer großen Asset-Bibliothek (Substance 3D Assets)

## Legende

| Zeichen | Bedeutung |
|---|---|
| `P0` | **Kern.** Ohne das ist das Programm für echte Arbeit nicht nutzbar. Gehört in die erste brauchbare Version. |
| `P1` | **Volle Alltags-Parität** mit Painter. |
| `P2` | **Fortgeschritten oder selten** gebraucht. Kommt später. |
| **＋** | Geht über Painter hinaus. |
| `[x]` | Fertig |

---

## 1. Projekt und Dateien

- [x] `P0` **Neues Projekt** (Strg+N): Dialog mit Mesh-Datei, Dokument-Auflösung und
      Normal-Map-Format (OpenGL / DirectX). Das Feld *Template* kommt mit den
      Projekt-Templates. Ein Mesh auf das Fenster ziehen öffnet den Dialog auch.
- [x] `P0` **Projekt speichern und öffnen** (Strg+S, Strg+O, *Save as*, *Save as copy*) in
      einem eigenen Format (`.ink3d`): eine Datei mit Mesh, Layern, Strokes, Bildern und
      Pinsel-Alphas, die sich ohne die Originaldateien öffnen lässt. Bevor ungespeicherte
      Änderungen verloren gehen, wird gefragt; der Fenstertitel zeigt sie mit einem Stern.
- [x] `P0` Zuletzt geöffnete Projekte (*Recent files*), Projekt schließen (Strg+F4).
- [x] `P1` **Autosave:** alle 10 Minuten eine Sicherung, solange es ungespeicherte
      Änderungen gibt, neben dem Projekt (die letzten 3; vor dem ersten Speichern im
      Datenordner). Nach einem Absturz öffnet man die Sicherung, wie in Painter.
- [ ] `P1` Autosave-Intervall und Anzahl der Sicherungen einstellen.
- [ ] `P1` **Projekt-Templates:** Shader, Kanäle und Auflösung als Vorlage speichern.
- [ ] `P1` **Mesh neu importieren** (Project configuration) mit Erhalt der Bemalung, wahlweise
      über UVs oder räumlich neu projiziert. Texture Sets werden über ihren Namen neu
      zugeordnet; nicht zuordenbare Sets werden als *disabled* markiert und lassen sich
      manuell zuweisen (Texture Set reassignment).
- [ ] `P1` Gebackene Mesh Maps beim Anlegen des Projekts mit importieren.
- [ ] `P2` Externe Ressourcen (Texturen, Mesh) bei Änderung automatisch aktualisieren.
- [ ] `P2` Automatisches UV-Unwrap für Meshes ohne UVs.
- [ ] `P2` Kameras aus der Mesh-Datei übernehmen.

**Mesh-Formate**

- [x] `P0` FBX (binär und ASCII), glTF / GLB, OBJ mit Achsen- und Einheitenkonvertierung
- [ ] `P2` USD, Alembic

---

## 2. Meshes, Texture Sets und Materialzuweisung

**Begriffe:**
- Ein **Mesh** ist ein Objekt aus der Modelldatei, z. B. „Torso“ oder „Gürtel“.
- Ein **Texture Set** entsteht aus einem **Material-Slot** des Modells. Jedes Texture Set
  hat einen eigenen Layer Stack, eigene Texturen und eine eigene Auflösung.
- Ein Mesh kann mehrere Materialien haben, und ein Material kann auf mehreren Meshes liegen.

**Zwei Wege, einem Mesh mehrere Materialien zu geben.** Beide lassen sich kombinieren.

| | Über das Modell (Texture Sets) | Über Masken im Layer Stack (Kapitel 5.1) |
|---|---|---|
| Festgelegt | im 3D-Programm, pro Polygon | in inkpainter3d, frei per Maske |
| Grenze | harte Kante an Polygongrenzen | beliebig, auch weich |
| Export | eigene Texturdateien pro Material | alles in denselben Texturen |
| Typisch | Haut, Kleidung, Glas | Stahl, Lack, Rost und Schmutz auf derselben Fläche |

### 2.1 Import

- [x] `P0` Beim Import bleiben **Materialzuweisung und Mesh-Namen erhalten**: aus FBX
      (Material pro Polygon), glTF (Material pro Primitive) und OBJ (`usemtl`).
- [x] `P0` Jedes Material wird ein Texture Set. Meshes ohne Material landen in einem
      Standard-Set.
- [x] `P0` **Tangenten nach MikkTSpace**, damit Normal Maps in Unreal, Unity, Godot und Blender
      genauso aussehen wie in inkpainter3d.

### 2.2 Texture-Set-Liste

So soll das Fenster aussehen. Painter zeigt nur die Texture Sets; die aufklappbaren Meshes
darunter sind unsere Erweiterung (**＋**):

```
Texture Sets                              [Menü ▾] [◎ Fokus]
▾ 👁  Body_MAT          2048²   [Shader: Standard ▾]
      👁  Torso                 12 400 Dreiecke
      👁  Arme                   8 200 Dreiecke
▾ 👁  Metall_MAT        1024²   [Shader: Standard ▾]
      👁  Gürtelschnalle         1 100 Dreiecke
      👁  Arme   ⧉               2 300 Dreiecke   ← Mesh hat mehrere Materialien
▸ ⊘  Glas_MAT  (ausgeblendet)
```

Wie in Painter:

- [x] `P0` **Ein Auge pro Texture Set** blendet das Set im Viewport ein und aus.
- [x] `P0` Menü mit **Alle zeigen**, **Alle ausblenden** und **Sichtbarkeit umkehren**.
- [x] `P0` **Fokus-Modus** (Alt+Q): Nur das aktive Set ist sichtbar, ein zweiter Klick hebt
      das wieder auf.
- [x] `P0` **Auswahl:** Genau ein Set ist aktiv. Layer Stack, Texture Set Settings und Shader
      Settings zeigen immer das aktive Set.
- [x] `P0` **Set im Viewport wählen:** Strg+Alt+Rechtsklick auf das Modell aktiviert das Set
      unter dem Mauszeiger.
- [ ] `P1` **Umbenennen** per Doppelklick (der Name wird beim Export verwendet), auf den
      Originalnamen zurücksetzen, eine Beschreibung pro Set.
- [ ] `P1` **Shader-Instanz pro Set:** Standardmäßig teilen sich alle Sets einen Shader; per
      „Neue Shader-Instanz“ bekommt ein Set einen eigenen.
- [ ] `P1` Einstellung „Beim Malen nur das aktive Set anzeigen“ (in Painter standardmäßig an,
      spart Leistung).

Über Painter hinaus (**＋**). Painter kann einzelne Meshes innerhalb eines Texture Sets nicht
ausblenden; bei Adobe liegt dazu ein offener Feature-Wunsch.

- [x] `P0` ＋ **Jedes Set lässt sich aufklappen** und zeigt die Meshes, die dieses Material
      benutzen, mit Dreiecksanzahl.
- [x] `P0` ＋ **Ein Auge pro Mesh** blendet einzelne Meshes aus, auch innerhalb eines Sets.
      Ausgeblendete Meshes werden nicht bemalt, sodass man darunter malen kann.
- [x] `P0` ＋ **Hervorheben beim Überfahren:** Fährt man in der Liste über ein Set oder Mesh,
      leuchtet es im Viewport auf.
- [ ] `P1` ＋ **Umgekehrte Ansicht „Nach Mesh“:** Mesh → seine Materialien. Meshes mit mehreren
      Materialien bekommen ein Symbol (⧉).
- [x] `P1` ＋ **Material-ID-Ansicht im Viewport:** Jedes Texture Set erscheint in einer eigenen
      Farbe, damit man auf einen Blick sieht, was wozu gehört.
- [x] `P1` ＋ **Info unter dem Mauszeiger:** In der Statusleiste stehen Mesh-Name und Texture Set
      unter dem Cursor; das UV-Tile kommt mit dem UDIM-Workflow dazu.
- [ ] `P2` ＋ **Material im Programm neu zuweisen:** ein Mesh in ein anderes Texture Set
      verschieben, ohne zurück ins 3D-Programm zu müssen.

### 2.3 Texture Set Settings

- [ ] `P0` **Auflösung** pro Set von 128 bis 4096, auch nicht quadratisch (z. B. 2048×1024).
      Die Auflösung ist jederzeit verlustfrei änderbar (siehe
      [Architektur-Entscheidungen](#architektur-entscheidungen)). `P1`: 8192.
- [ ] `P0` **Kanalliste:** Kanäle hinzufügen und entfernen. Entfernte Kanäle behalten ihre
      Daten und können wieder hinzugefügt werden.
- [ ] `P1` **Format pro Kanal:** sRGB8, L8, RGB8, L16, RGB16, L16F, RGB16F, L32F, RGB32F.
- [ ] `P1` **Normal mixing:** *Combine* (Standard) oder *Replace* mit der gebackenen Normal Map.
- [ ] `P1` **Height-zu-Normal-Methode:** *Smooth (Sobel)* (Standard) oder *Sharp*.
- [ ] `P1` **AO mixing:** *Multiply* (Standard) oder *Replace* mit der gebackenen AO.
- [ ] `P1` **UV Padding:** *3D Space Neighbor* (Standard) oder *2D Space Neighbor*.
- [ ] `P1` **Mesh-Map-Slots** (Normal, World Space Normal, ID, AO, Curvature, Position,
      Thickness, Height …), befüllbar per Drag & Drop oder per „Bake Mesh Maps“.

### 2.4 Geometry Mask und UV-Tiles

- [ ] `P1` **Geometry Mask pro Layer oder Ordner:** Ein- und Ausschlussliste nach Mesh-Namen
      oder UV-Tiles. Rechtsklick zum Kopieren, Einfügen, Alles ein- oder ausschließen.
- [ ] `P1` **Ausgeschlossene Geometrie ausblenden** (Alt+H), um darunter zu malen.
- [ ] `P1` **UV-Tile-Workflow (UDIM):** mehrere UV-Tiles pro Texture Set. Das Datenmodell muss
      das von Anfang an vorsehen, auch wenn die Oberfläche erst später kommt.

---

## 3. Viewport und Darstellung

**Ansichten**

- [x] `P0` 3D-Ansicht
- [x] `P0` **2D-Ansicht** (UV-Layout), in der man ebenfalls malen kann. Navigation wie in 3D,
      Alt+Links dreht die Ansicht. Striche aus der 2D-Ansicht bleiben im UV-Raum, wie Painters
      Alignment „UV“; Painters Standard (Tangent Wrap) überträgt sie auf die Oberfläche und
      folgt mit den Alignment-Optionen (Kapitel 8).
- [x] `P0` Layouts **F1** 3D+2D, **F2** nur 3D, **F3** nur 2D, **F4** 3D und 2D tauschen
- [x] `P1` Kamera **perspektivisch (F5)** oder **orthografisch (F6)**
- [ ] `P1` Brennweite / FOV einstellbar
- [ ] `P1` UI ausblenden (Tab)

**Beleuchtung**

- [x] `P0` **HDRI-Umgebung (Image Based Lighting).**
- [x] `P0` **Umgebung drehen mit Shift+Rechts ziehen**, dazu Belichtung,
      Hintergrund-Deckkraft und Hintergrund-Unschärfe.
- [x] `P0` Mitgelieferte HDRIs (CC0, z. B. von Poly Haven)
- [ ] `P1` Schatten
- [ ] `P1` Viewport-Ambient-Occlusion

**Anzeige-Modi**

- [x] `P0` **Material (M):** fertiges Material
- [x] `P0` **Kanal (C / Shift+C):** einzelnen Kanal durchschalten, z. B. nur Roughness
- [ ] `P1` **Mesh Map (B / Shift+B):** gebackene Maps durchschalten
- [ ] `P1` Maske eines Layers anzeigen (Alt+Klick auf die Maske)
- [x] `P1` ＋ Wireframe des Meshes als Overlay, einzeln ein- und ausschaltbar in der 3D- und in der 2D-Ansicht (Schalter neben dem Anzeige-Modus, standardmäßig aus). UV-Nähte folgen.

**Bildnachbearbeitung**

- [x] `P0` Tone Mapping (ACES)
- [ ] `P1` Anti-Aliasing (TAA)
- [ ] `P1` Farbkorrektur
- [ ] `P2` Glow / Bloom, Depth of Field (Autofokus mit Strg+Mitte), Vignette,
      Linsenverzerrung, Lens Flares, Film Grain
- [ ] `P2` Farbmanagement mit OCIO / ACES

**Rendern**

- [ ] `P2` Path-Tracing-Modus (F10) für Präsentationsbilder. Painter nutzt dafür NVIDIA Iray;
      wir bräuchten einen eigenen Path Tracer.

---

## 4. Kanäle und Shader

**Kanäle**

| Prio | Kanäle |
|---|---|
| `P0` | Base Color, Roughness, Metallic, Normal, Height, Emissive, Opacity |
| `P1` | Ambient Occlusion, Specular / Glossiness (für Spec-Gloss), User-Kanäle (frei benennbar, mehrere) |
| `P2` | Specular Level, Displacement, Anisotropy, Transmission, IOR, Scattering (SSS), Sheen, Coat |
| **＋** | **Ink** (Linienfarbe und -stärke für den Stylized-Look) |

Die Farbbehandlung richtet sich nach der Rolle des Kanals: Base Color ist sRGB, Roughness ist
lineare Daten. Bei User-Kanälen ist sie umschaltbar.

**Shader**

- [x] `P0` **PBR Metallic/Roughness** (GGX)
- [x] `P0` ＋ **Stylized / Toon** mit Ink-Outline. Das Grundgerüst steht, wird aber erweitert.
- [ ] `P1` **PBR Specular/Glossiness**
- [x] `P0` Opacity: Alpha Test und Alpha Blend
- [ ] `P1` Unlit
- [ ] `P1` Shader-Parameter pro Shader-Instanz (siehe 2.2)
- [ ] `P2` Subsurface Scattering
- [ ] `P2` Coat / Sheen im Stil von OpenPBR

---

## 5. Layer Stack

### 5.1 Materialien stapeln (Grundprinzip)

Jedes Texture Set hat einen Layer Stack. Ein Material darauf ist ein **Fill Layer** mit
optionaler **Maske**. Die Reihenfolge im Stack ist die Priorität: **Das oberste Material
gewinnt**, aber nur dort, wo seine Maske es zulässt.

```
Layer Stack von Body_MAT                 Ergebnis
  3  Rost         [Maske]                Rost, wo seine Maske weiß ist
  2  Roter Lack   [Maske]                Lack, wo seine Maske weiß ist und kein Rost liegt
  1  Stahl        (keine Maske)          Stahl überall sonst
```

- [x] `P0` **Ohne Maske** deckt ein Material das ganze Texture Set ab. Alles darunter ist
      verdeckt.
- [x] `P0` **Mit Maske:** Weiß zeigt dieses Material, Schwarz lässt die Materialien darunter
      durch, Grau mischt beide. Graustufen ergeben weiche Übergänge, z. B. abgeplatzten Lack
      an Kanten.
- [x] `P0` **Priorität ändern** durch Verschieben im Stack per Drag & Drop.
- [x] `P0` **Pro Kanal:** Ein Material gewinnt nur in den Kanälen, die es eingeschaltet hat.
      Ein „Nass“-Layer, der nur Roughness setzt, lässt die Farbe darunter stehen.
- [x] `P0` **Deckkraft und Blend Mode** verändern das Ergebnis: Bei *Normal* mit 100 % gilt
      „oben gewinnt“, bei weniger Deckkraft oder anderen Modi wird gemischt.
- [x] `P0` **Ordner mit Maske:** Mehrere Layer lassen sich gruppieren und gemeinsam maskieren,
      z. B. „Lack“ zusammen mit „Kratzer im Lack“.
- [ ] `P1` **Woher Masken kommen:** gemalt, Polygon Fill (Mesh, Polygon, UV-Insel), Geometry
      Mask (nach Mesh-Name), Color Selection aus der ID-Map, Generatoren wie Edge Wear oder
      Dirt.
- [ ] `P1` **Material auf ein Teil ziehen:** Strg+Drag & Drop eines Materials auf das Modell
      erzeugt einen Fill Layer, dessen Maske nur das Teil unter dem Mauszeiger abdeckt.
- [ ] `P2` ＋ **„Nach Mesh“-Ansicht** der Texture-Set-Liste zeigt pro Mesh, welche Material-Layer
      darauf wirken.

### 5.2 Layer

**Layer-Typen**

- [x] `P0` **Paint Layer**
- [x] `P0` **Fill Layer**
- [x] `P0` **Ordner (Gruppe)**, auch als *Passthrough*
- [ ] `P2` **Instanz-Layer:** ein Layer, der in mehreren Texture Sets gleichzeitig wirkt
- [ ] `P2` **Layer zusammenführen (Flatten)**, auch als Export auf die Festplatte

**Eigenschaften pro Layer**

- [x] `P0` **Blend Mode und Deckkraft pro Kanal.** Wie in Painter wählt ein Dropdown oben im
      Layer Stack den Kanal, dessen Blending man gerade bearbeitet.
- [x] `P0` Kanäle pro Layer ein- und ausschalten (z. B. Layer wirkt nur auf Roughness)
- [x] `P0` Sichtbarkeit, Umbenennen (Doppelklick), Reihenfolge per Drag & Drop, auch in Ordner
      hinein; Mehrfachauswahl mit Strg+Klick
- [ ] `P1` Vorschaubilder für Layer und Masken. Bisher zeigen die Zeilen Symbole, die Farbe
      eines Fill Layers und den Grundton einer Maske.

**Blend Modes**

- [x] `P0` Normal, Passthrough, Disable, Replace, Multiply, Screen, Overlay,
      Linear Dodge (Add), Subtract, Normal Map Combine, Normal Map Detail
- [x] `P1` die übrigen Modi aus Painter: Divide, Inverse Divide, Darken (Min), Lighten (Max),
      Inverse Subtract, Difference, Exclusion, Signed Addition, Linear Burn, Color Burn,
      Color Dodge, Soft Light, Hard Light, Vivid Light, Linear Light, Pin Light, Tint,
      Saturation, Color, Value, Normal Map Inverse Detail

**Bearbeiten**

- [x] `P0` Kopieren (Strg+C), Ausschneiden (Strg+X), Einfügen (Strg+V), Löschen (Entf),
      Duplizieren (Strg+D), Gruppieren (Strg+G). Kopieren, Ausschneiden, Einfügen und Löschen
      wirken wie in Painter nur, wenn die Maus über dem Layer Stack steht.
- [ ] `P1` Layer zwischen Texture Sets kopieren
- [ ] `P1` Nur den Layer-Inhalt kopieren und einfügen (Strg+Shift+C / V)
- [ ] `P2` Material Layering (Unter-Stacks pro Texture Set)

---

## 6. Masken und Effekte

**Masken**

- [x] `P0` **Schwarze oder weiße Maske** hinzufügen und hineinmalen; in der Maske malt der Pinsel
      Graustufen, X tauscht Schwarz und Weiß
- [x] `P1` Maske ein- und ausschalten (Shift+Klick)
- [ ] `P1` Maske im Viewport anzeigen (Alt+Klick)
- [x] `P1` **Color Selection:** Masken-Effekt, der die Teile mit den gewählten Farben der
      gebackenen ID-Map weiß macht. „Pick color“, dann Klick auf das Modell (3D oder 2D)
      nimmt die Farbe darunter auf, bis zu 16 Farben; Tolerance, Hardness und Invert wie in
      Painter. Ohne ID-Map wählt sie nichts aus.
- [x] `P1` **Smart Masks:** gespeicherte Masken-Aufbauten aus dem Shelf
- [ ] `P2` **Quick Mask** (U bearbeiten, Y leeren, I invertieren)

**Effekte.** Sie liegen in der Maske und auch im Inhalt eines Layers und bilden dort einen
eigenen Stapel. *Seit Phase 4* gibt es Effekte in Masken, jeder mit Blend Mode und
Deckkraft; Effekte im Layer-Inhalt folgen mit den Filtern.

- [x] `P0` **Paint**
- [x] `P0` **Fill**
- [x] `P1` **Levels**
- [ ] `P1` **Filter** (siehe Kapitel 11). Blur gibt es in Masken; im Layer-Inhalt fehlen
      Filter noch.
- [x] `P1` **Generator** (siehe Kapitel 11), bisher nur in Masken
- [ ] `P2` **Compare Mask**
- [ ] `P2` **Anchor Point:** Zwischenergebnis des Stacks, das spätere Generatoren und Filter
      als Eingabe nutzen. Klassisches Beispiel: Kanten-Abnutzung, die gemalte Height-Details
      berücksichtigt.

---

## 7. Werkzeuge

Taste in Klammern = Painter-Shortcut.

**Malen und Füllen**

- [x] `P0` **Paint (1)**
- [x] `P0` **Eraser (2)**
- [ ] `P1` **Projection (3):** malt mit einer projizierten Textur, die sich mit W / E / R
      verschieben, drehen und skalieren lässt
- [x] `P1` **Polygon Fill (4):** füllt nach Dreieck, Polygon, Mesh oder UV-Insel, im 3D- und im
      2D-Fenster. Ideal für Masken. Das Polygon ist eine Näherung: Das Mesh ist trianguliert,
      darum gilt ein Dreieck samt seiner Nachbarhälfte (gleiche Ebene, gleiche längste Kante)
      als Viereck. Ein Fill ist ein Strich ohne Tupfer und speichert seine Dreiecke.
- [ ] `P1` **Pick Material (P):** übernimmt alle Kanalwerte vom Modell
- [ ] `P2` **Smudge (5)**
- [ ] `P2` **Clone (6 relativ, Strg+6 absolut):** Quelle mit V+Klick setzen

**Strich-Hilfen**

- [x] `P0` **Gerade Linien:** Shift+Klick zieht eine Linie vom Ende des letzten Strichs, mit
      Strg+Shift+Klick auf 15° eingerastet. Eine gestrichelte Hilfslinie zeigt sie vorher.
- [x] `P0` **Lazy Mouse (D):** Strichglättung. Der Pinsel hängt an einer Schnur hinter dem
      Mauszeiger; Abstand in Prozent der Ansichtshöhe, Standard 8.
- [x] `P1` **Symmetrie (L):** Spiegeln an X / Y / Z (auch kombiniert) oder radial um eine Achse,
      durch einen frei wählbaren Mittelpunkt (Weltursprung oder Modellmitte). Jede Kopie malt
      mit einer ebenso gespiegelten Kamera, so wirkt die Verdeckung auch auf der Rückseite.
      Der Cursor zeigt die Kopien. Gilt für Striche im 3D-Fenster; Polygon Fill und Striche
      im 2D-Fenster bleiben, wie gemalt. Es fehlen noch: Spiegeln im lokalen Raum eines
      Meshes und eine sichtbare Symmetrie-Ebene im Viewport.
- [ ] `P1` **Vorherigen Strich fortsetzen (A)**
- [x] `P1` **Stencil:** Maske über dem Viewport; S+Links drehen, S+Mitte verschieben,
      S+Rechts zoomen, N ignoriert die Stencil-Maske
- [x] `P1` **Graustufen invertieren (X)**

**Pfad-Werkzeuge**

- [ ] `P2` **Paint along path:** Pfad mit Kontrollpunkten auf der Oberfläche, nachträglich
      editierbar
- [ ] `P2` **Filled path:** gefüllte Fläche aus einem Pfad
- [ ] `P2` **Ribbon path:** verformt ein Bild entlang des Pfads (z. B. Nähte)

**Sonstige**

- [ ] `P2` **Partikel-Pinsel** (Strg+1 / 2 / 3): physikalisch simulierte Partikel, die Spuren
      hinterlassen (Regen, Rost-Läufe)

---

## 8. Pinsel-Eigenschaften

**Form und Strich**

- [x] `P0` **Size:** Strg+Rechts horizontal ziehen oder [ und ]. Der Größenraum ist wählbar:
      Object (relativ zum Modell, Standard) oder Viewport. Painters dritte Option „Texture“
      (an den 2D-Zoom gekoppelt) fehlt noch.
- [x] `P0` **Hardness:** Strg+Rechts vertikal ziehen
- [x] `P0` **Flow:** Strg+Links horizontal ziehen
- [x] `P0` **Stroke Opacity**
- [x] `P0` **Spacing**
- [x] `P1` **Angle:** Strg+Links vertikal ziehen, dazu **Follow Path** (das Alpha dreht sich in Strichrichtung)
- [x] `P1` **Jitter:** Size, Flow, Angle, Position. Die Zufallswerte hängen an Strich-ID und Tupfer-Nummer, ein Strich sieht beim erneuten Zeichnen identisch aus.

**Projektion**

- [x] `P0` **Backface Culling** mit Winkel, gemessen zur Normale des Stempels wie in Painter
- [x] `P0` **Verdeckte Flächen bleiben frei:** Jeder Strich merkt sich seine Kamera; was von dort
      hinter anderen Flächen liegt, bekommt keine Farbe.
- [x] `P1` **Alignment:** Tangent Wrap (Standard), Tangent Planar, Camera. *UV* (Alpha folgt der UV-Richtung) fehlt noch.

**Tablet**

- [x] `P0` **Stiftdruck** auf Size und Flow, je einzeln schaltbar, mit Mindestwerten und
      Druckkurve (Linear, Ease In, Ease In Out). Wie in Painter wirkt Druck nicht auf die
      Stroke Opacity.
  - Windows: Windows Ink, wie Painter.
  - Linux mit Wayland (GNOME, KDE): eigenes Tablet-Protokoll, denn diese Desktops geben Stifte
    nur so weiter. Das Radierer-Ende des Stifts schaltet auf den Eraser.
  - Linux mit X11: Der Stift bewegt die Maus, den Druck liest eine eigene XInput2-Verbindung.
- [ ] `P1` **Stift-Neigung (Tilt)** ＋ (Painter unterstützt sie nicht)

**Spitze und Material**

- [x] `P0` **Alpha:** Pinselform aus einem Graustufenbild (PNG, JPEG; Bilder mit Transparenz
      nutzen den Alphakanal). Dazu runde und eckige Spitzen mit Härte.
- [x] `P1` **Stencil:** Bild laden (Eigenschaften), Größe, Drehung, Overlay; der Strich merkt sich die
      Abbildung vom Bild auf den Bildschirm (3D) oder die UV-Fläche (2D) und wirkt je Tupfer, damit
      Live-Strich und Neuzeichnen gleich aussehen.
- [x] `P0` **Material des Pinsels:** Wert oder Farbe pro Kanal; Kanäle einzeln ein- und
      ausschaltbar. Der Eraser hat eigene Kanal-Schalter.
- [ ] `P1` Material des Pinsels aus Textur oder Material aus dem Shelf (ein Material lässt sich
      bisher nur als Fill Layer anlegen)

**Presets**

- [x] `P1` **Pinsel-Presets** speichern und laden: sieben mitgelieferte Presets (Soft round, Hard round, Airbrush, Marker, Chalk, Scatter dots, Decal stamp) und eigene als `.inkbrush`-Datei (ZIP aus `brush.json` und `alpha.png`) im Bibliotheksordner. Das Material (Kanäle, Werte) bleibt beim Laden erhalten.

---

## 9. Fill Layer und Projektionen

**Projektionsarten**

- [x] `P0` **UV-Projektion:** Kachelung, Versatz, Rotation
- [x] `P0` **Tri-planar:** nahtlos über UV-Nähte, mit einstellbarer Kantenhärte
- [ ] `P1` **Planar**
- [ ] `P1` **Sphärisch**
- [ ] `P1` **Zylindrisch**
- [ ] `P1` **Fill (match per UV Tile)**
- [ ] `P2` **Warp-Projektion:** Projektionsbox mit verschiebbaren Ecken, mit Automatik-Modus
- [ ] `P2` **UV-Set zu UV-Set**

**Füllinhalt**

- [x] `P0` Fester Wert oder Farbe pro Kanal
- [x] `P0` Bild-Textur pro Kanal (PNG, JPEG, TGA, BMP, EXR, HDR). Normal Maps gelten wie in
      Painter als DirectX, umschaltbar auf OpenGL; Height wird zu Normal-Details.
- [ ] `P1` Material aus dem Shelf, prozedurale Texturen
- [ ] `P1` Material per Drag & Drop direkt auf den Layer Stack oder das Modell ziehen
      (Strg+Drag wirkt nur auf das Teil unter dem Mauszeiger)

---

## 10. Baking (Mesh Maps)

- [x] `P1` **Baking-Fenster** (Strg+Shift+B, auch im Datei-Menü). Ein frei verschiebbares Fenster, der Bake
      läuft in einem Hintergrund-Thread mit Fortschritt und Abbrechen. Der Baking-Modus (F8)
      fehlt.

**Maps**

| Prio | Maps | Hinweis |
|---|---|---|
| `P1` ✅ | Ambient Occlusion, Curvature, World Space Normal, Position, Thickness | vom Low-Poly-Mesh selbst. Die Generatoren brauchen sie. |
| `P1` ✅ | Normal (Tangent Space), Height, ID | von High-Poly auf Low-Poly; ID aus Material oder Mesh-Objekt des High-Poly |
| `P2` | Bent Normals, Opacity | |

Die Maps liegen im Texture Set (16 Bit, in der Größe des Texture Sets) und im Projekt. Das
Fenster zeigt Vorschaubilder; *Add as layer* legt eine Map als Fill Layer an, zum Ansehen.

**Einstellungen**

- [x] `P1` Ausgabegröße (die des Texture Sets), Dilation, Antialiasing (1×1, 2×2, 4×4)
- [x] `P1` High-Poly-Dateien (mehrere, werden beim Bake geladen und zusammengefasst)
- [x] `P1` Zuordnung *Always* oder *By mesh name* (`name_low` sieht nur `name_high`; Meshes
      ohne Suffix sehen alles)
- [x] `P1` Max. Frontal- und Rear-Distanz (relativ zur Diagonale der Bounding Box), gemittelte
      Normalen, Backfaces ignorieren. Eine Cage-Datei fehlt noch.
- [x] `P1` Auswahl, welche Texture Sets gebacken werden
- [x] `P1` Einstellungen pro Map: AO-Strahlen, -Distanz und Self-Occlusion, Thickness-Strahlen und
      -Distanz, Curvature-Radius, ID-Quelle. Eine Ground Plane gibt es nicht.

**Technik**

- [x] `P1` GPU-Raytracing über eine eigene BVH im Compute-Shader (`bake.wgsl`). Das läuft auf
      jeder Vulkan-GPU und braucht keine RT-Kerne. Das Texture Set wird in Kacheln zu 256 × 256
      gebacken: Ein Raster-Pass zeichnet pro Kachel die Dreiecks-ID jedes Texels, der Compute-Shader
      schießt von dort Strahlen. Die Curvature kommt aus einem Ring von Strahlen um den Punkt (wie
      weit liegt die Fläche über oder unter der Tangentialebene), so sieht sie auch harte Kanten.
- [ ] `P2` Automatisch neu backen, wenn sich das Mesh ändert

---

## 11. Generatoren, Filter, Smart Materials

Painter liefert diese Bausteine als `.sbsar`-Dateien aus, die nur Adobes Engine ausführen kann.
**Wir bauen sie selbst:** als WGSL-Shader mit einer Parameter-Beschreibung (siehe
[Architektur-Entscheidungen](#architektur-entscheidungen)).

**Generatoren** (nutzen die Mesh Maps)

Ein Generator ist ein Masken-Effekt (Effekt-Menü der Ebene). Jede Art beschreibt ihre
Parameter einmal (`ink_core::generator`): Daraus entstehen die Oberfläche, die Projektdatei
und die Slots des Shaders (`generators.wgsl`). Ohne gebackene Maps laufen sie weiter, mit
Rauschen statt Kanten und Spalten; Rauschen liegt mit einer Position-Map im Objektraum,
sonst im UV-Raum.

- [x] `P1` **Mask Builder:** mischt Curvature, AO, Thickness und Höhe mit Rauschen, dazu Kontrast
      und Balance
- [x] `P1` **Edge Wear:** Kanten-Abnutzung aus der konvexen Curvature, vom Rauschen aufgebrochen
- [x] `P1` **Dirt:** Schmutz aus AO und konkaver Curvature, Staub auf nach oben zeigenden Flächen
- [x] `P1` **Position Gradient:** Verlauf entlang X, Y oder Z der Bounding Box, mit Rauschen
- [x] `P1` **Light:** gerichtete Beleuchtung aus der World-Normal-Map (Azimut, Höhe)
- [x] `P1` Curvature (konvex, konkav oder beides), AO und Thickness einzeln als Generator

**Filter**

Filter sind Masken-Effekte wie *Levels*. Wer Nachbartexel liest, braucht einen Rand um jede
Kachel des Compositors: Er wächst mit der Reichweite der Filter einer Maske, und beim Malen
unter einem Filter wird um diese Reichweite mehr neu berechnet.

- [x] `P0` **Blur** (Masken-Effekt, Menü *Filters*): Gauß-Blur im Texturraum, mit Blend Mode
      und Deckkraft wie jeder Effekt. *Intensity* 0 bis 1 hängt nicht von der Auflösung ab;
      volle Stärke reicht 3 % der Texturbreite weit, höchstens 128 Texel. Wie in Painter
      blurt er über UV-Nähte hinweg in den Rand der Inseln. Im Layer-Inhalt fehlt er noch.
- [ ] `P1` Sharpen, HSL / Farbkorrektur, Gradient Map, Height-zu-Normal
- [ ] `P2` Directional Blur, Slope Blur, Warp, Baked Lighting
- [ ] `P2` ＋ Stylized-Filter: Posterize, Hand-Painted-Look, Pixel Art

**Prozedurale Texturen**

- [x] `P1` Noises (Perlin, Fractal, Zellen, Wolken, Zellränder für Risse und Falten) als Generator *Noise*
- [x] `P1` Grunges (Kratzer, Haarstriche, Risse, Flecken, Staub) als Alphas im Shelf, im Code erzeugt
- [ ] `P1` Muster (Kacheln, Ziegel, Streifen)

**Presets**

- [x] `P1` **Smart Materials:** ausgewählte Ebenen als Preset speichern (`.inksmart`, ein ZIP aus
      JSON und Bildern; gemalte Striche gehören nicht dazu). Klick oder Ziehen auf den Layer
      Stack legt Kopien mit neuen Ids über die ausgewählte Ebene. Was auf Generatoren beruht,
      passt sich über die Mesh Maps an jedes Modell an.
- [x] `P1` **Smart Masks:** Maske der ausgewählten Ebene als Preset; ersetzt die Maske der
      ausgewählten Ebene

**Mitgelieferte Inhalte**

- [x] `P1` **Starter-Bibliothek:** zwölf Smart Materials (Steel, Painted metal, Rusty metal, Gold,
      Copper patina, Glossy plastic, Rubber, Dry wood, Leather, Rough stone, Concrete, Fabric) und
      sechs Smart Masks (Edge wear, Dirt in crevices, Worn surface, Noise patches, Gradient from the
      bottom, Dust on top), sieben Pinsel, acht Alphas, die HDRI. Alles im Code erzeugt. Jedes
      Material formt auch die Oberfläche über den Height-Kanal: Lackschicht, Rostkruste,
      Lochfraß, Poren, Risse, Lederkörnung, Fasern.
      Weitere Materialien kommen aus den Online-Bibliotheken (CC0).

---

## 12. Shelf (Asset-Bibliothek)

Das Shelf ist ein andockbares Fenster (standardmäßig unter dem Viewport) mit den Kategorien
Brushes, Alphas, Textures, Materials und Online. Die eigene Bibliothek liegt im Datenordner
(`shelf/brushes`, `alphas`, `textures`, `materials`), der Ordner-Knopf öffnet sie.

- [x] `P1` **Kategorien** und Vorschaubilder. Umgesetzt: Brushes (Presets mit Vorschaustrich),
      Alphas (acht im Code erzeugte plus eigene), Textures, Materials, Smart Materials, Smart
      Masks, Online. Procedurals, Filters, Environments, Export Presets und Templates fehlen noch.
- [x] `P1` Suche (Filter) und Drag & Drop: Materials und Textures lassen sich auf den Layer Stack
      ziehen und werden zum Fill Layer. Brushes und Alphas wirken per Klick. Drop auf Viewport und
      Parameterfelder fehlt noch.
- [x] `P1` **Eigene Ressourcen importieren** (Pinsel-Presets, Alphas, Texturen, Materialien) in die
      Bibliothek. Rechtsklick: Löschen, Alpha als Pinselform oder Stencil nutzen. Nur-im-Projekt
      gibt es nicht.
- [ ] `P2` Eigene Bibliotheks-Ordner (Sub-Libraries), Favoriten, Tags
- [x] `P1` ＋ **PBR-Materialordner importieren:** ein Ordner mit Base Color, Roughness, Normal …
      im Format von Poly Haven oder ambientCG wird zu einem `.inkmat`-Material (ZIP aus
      `material.json`, den Karten und `preview.png`). Erkannt werden die Dateinamen beider
      Seiten; ARM- und AO-Karten bleiben außen vor, weil es keinen AO-Kanal gibt.
- [x] `P1` ＋ **Online-Bibliotheken im Shelf:** ambientCG und Poly Haven (zusammen mehrere
      tausend CC0-Materialien) durchsuchen, Vorschau, Download in 1K, 2K oder 4K in die
      Bibliothek und als Fill Layer anlegen. Das ist der Ersatz für Substance 3D Assets, die nur mit
      Adobes Engine laufen.

---

## 13. Export

- [x] `P0` **Export-Dialog** (Strg+Shift+E) wie Painters: Zielordner, Vorlage, Dateityp,
      Padding, Auswahl der Texture Sets (*Check all*, *Uncheck all*, *Invert all*), Vorschau
      der Dateien pro Texture Set und die *List of exports*. *Save settings* speichert die
      Einstellungen im Projekt.
- [x] `P0` **Mitgelieferte Vorlagen für Blender, Unity und Godot**, dazu glTF PBR Metal
      Roughness, PBR Metallic Roughness und Document Channels (mit und ohne Alpha). Alle
      drei Engines erwarten Normal Maps im OpenGL-Format; die Normal Map enthält die
      Details aus Height, wie in Painter.
  - Blender (Principled BSDF): Base Color, Metallic, Roughness, Normal, Emission, Alpha und
    Displacement als einzelne Dateien, so benannt, dass Node Wrangler sie verbindet
  - Unity HDRP: Base Map mit Opacity im Alpha, Mask Map (Metallic, AO, Detail Mask,
    Smoothness), Normal, Emissive
  - Unity URP und Built-in: Base Map / Albedo mit Opacity im Alpha, Metallic mit Smoothness
    im Alpha-Kanal, Normal, Occlusion, Emission
  - Godot (ORMMaterial3D): Albedo mit Alpha, ORM-Textur (Occlusion, Roughness, Metallic),
    Normal, Emission, Height
  - ＋ Wo kein Layer etwas hinterlässt, steht im Export, was der Viewport zeigt (die
    Standardwerte des Shaders), nicht Schwarz wie in Painter.
- [x] `P1` Vorlage für Unreal Engine (Packed), Normal Map im DirectX-Format
- [ ] `P1` Vorlagen für Arnold, USD
- [x] `P0` **Dateiformate** PNG, TGA, JPEG mit 8 Bit, auch mit Dithering gegen Banding
- [x] `P1` PNG und TIFF mit 16 Bit, OpenEXR mit 16 / 32 Bit Float (Farben linear)
- [ ] `P2` PSD mit Layern, DDS
- [x] `P0` **Padding:** *Dilation infinite*, *Dilation + transparent* und *Dilation + default
      background color* mit Abstand in Pixeln, *No padding (passthrough)*
- [ ] `P1` *Dilation + diffusion*: jenseits des Abstands eine unscharfe Fortsetzung
- [ ] `P1` **Vorlagen-Editor:**
  - Kanäle frei auf R / G / B / A packen
  - Umgerechnete Maps: Normal OpenGL ↔ DirectX, Glossiness aus Roughness, Mixed AO,
    Diffuse / Specular aus Metal-Rough
  - Dateinamen-Muster mit Platzhaltern wie `$mesh`, `$textureSet`, `$udim`
- [ ] `P1` Auflösung beim Export pro Texture Set überschreiben
- [ ] `P1` Export pro UV-Tile (UDIM)
- [ ] `P2` Mesh samt Texturen als glTF / GLB oder USD exportieren
- [ ] `P2` ＋ Export über die Kommandozeile für automatisierte Pipelines

---

## 14. Bedienung und Shortcuts

**Alle Shortcuts sind wie in Painter belegt und lassen sich ändern.** Die vollständige Liste:

| Bereich | Aktion | Shortcut |
|---|---|---|
| Kamera | Drehen | Alt+Links |
| | Drehen in 90°-Schritten | Alt+Shift+Links |
| | Verschieben | Alt+Mitte (oder Strg+Alt+Links) |
| | Zoomen | Alt+Rechts, Mausrad |
| | Modell einpassen | F |
| | Perspektivisch / Orthografisch | F5 / F6 |
| | Umgebung (HDRI) drehen | Shift+Rechts |
| Werkzeuge | Paint / Eraser / Projection / Polygon Fill / Smudge / Clone | 1 / 2 / 3 / 4 / 5 / 6 |
| | Material vom Modell aufnehmen | P |
| | Symmetrie | L |
| | Lazy Mouse | D |
| | Verschieben / Drehen / Skalieren (Projektion) | W / E / R |
| | Manipulator zeigen / Raum wechseln | Q / T |
| Pinsel | Größe | [ und ], Strg+Rechts horizontal |
| | Härte | Strg+Rechts vertikal |
| | Flow / Winkel | Strg+Links horizontal / vertikal |
| | Gerade Linie (eingerastet) | Shift+Links (Strg+Shift+Links) |
| Layer | Kopieren / Ausschneiden / Einfügen | Strg+C / X / V |
| | Duplizieren / Gruppieren / Löschen | Strg+D / Strg+G / Entf |
| Ansicht | Layouts | F1 bis F4 |
| | Material / Kanal / Mesh Map | M / C, Shift+C / B, Shift+B |
| | Texture Set isolieren | Alt+Q |
| | Ausgeschlossene Geometrie ausblenden | Alt+H |
| | Texture Set unter Maus wählen | Strg+Alt+Rechts |
| Allgemein | Rückgängig / Wiederholen | Strg+Z / Strg+Y |
| | Neu / Öffnen / Speichern | Strg+N / Strg+O / Strg+S |
| | Baken / Exportieren | Strg+Shift+B / Strg+Shift+E |
| | UI ausblenden | Tab |
| | Berechnung pausieren | Shift+Esc |

Umgesetzt sind Kamera, Umgebung, die Anzeige-Modi Material und Kanal (M, C / Shift+C),
Fokus-Modus, Set-Auswahl, Paint und Eraser (1 / 2), Pinselgröße, Härte, Flow und Winkel,
gerade Linien, Lazy Mouse, Symmetrie (L), Graustufen invertieren (X), die Layer-Shortcuts, Rückgängig /
Wiederholen und die Layouts F1 bis F4; Mesh Map
(B / Shift+B) folgt mit dem Baking. [ und ] funktionieren auch, wenn AltGr als Strg+Alt
ankommt (deutsche Tastatur unter Windows). Unter Linux
verschieben viele Desktops Fenster mit Alt + Ziehen; die README erklärt, wie man das auf die
Super-Taste umstellt. `P2` ＋: Alternative Navigationsschemata (Blender, Maya) als Option.

---

## 15. Oberfläche

- [x] `P1` **Andockbare Fenster:** Viewport, Texture-Set-Liste, Layers, Properties (zeigt je
      nach Auswahl Werkzeug, Layer oder Effekt), Texture Set Settings und Display Settings
      lassen sich verschieben, teilen, stapeln und als eigene Fenster abziehen; Menü Window.
      Shelf, Shader Settings, History und Log folgen mit ihren Funktionen.
- [x] `P1` Fenster-Layouts speichern und wiederherstellen; Standard-Layout ähnlich wie in
      Painter. Das Layout merkt sich das Programm beim Beenden; Window → Save layout und
      Load layout schreiben und lesen Dateien, Reset layout stellt das Standard-Layout her.
- [ ] `P1` Dunkles Design, skalierbar für HiDPI
- [ ] `P2` Deutsche Oberfläche (mit den Begriffen der deutschen Painter-Oberfläche). Die Oberfläche ist bewusst englisch, damit möglichst viele Menschen das Programm nutzen können; weitere Sprachen folgen im Feinschliff.

---

## 16. Undo, Leistung, Stabilität

- [ ] `P0` **Undo / Redo** für alle Aktionen (Striche, Layer, Einstellungen). Striche und alle
      Änderungen am Layer Stack sind umgesetzt (Strg+Z, Strg+Y, Strg+Shift+Z), Einstellungen
      folgen.
- [ ] `P1` **History-Fenster**
- [ ] `P0` **Flüssiges Malen bei 4K pro Texture Set** auf einer Mittelklasse-GPU
- [ ] `P0` **5 Texture Sets mit je 4K** in einem Projekt wie im Referenzprojekt
- [ ] `P1` **Speicherbudget:** Caches werden ausgelagert, bevor der VRAM voll ist
- [ ] `P0` Läuft unter **Vulkan** (Linux, Windows) und mit **Mesa** (AMD, Intel) sowie NVIDIA
- [ ] `P0` Tests und CI für Windows und Linux (besteht bereits)

---

## 17. Scripting und Erweiterbarkeit

- [ ] `P2` **Python-API:** Projekt öffnen, Layer anlegen, backen, exportieren; eigene Menüpunkte
- [ ] `P2` **Eigene Generatoren und Filter als Dateien:** WGSL-Shader plus Parameter-Beschreibung
      im Shelf-Ordner ablegen, fertig

---

## 18. Über Painter hinaus

Was inkpainter3d zusätzlich können soll:

| Funktion | Kapitel |
|---|---|
| Meshes innerhalb eines Texture Sets einzeln ausblenden; Baum Material ↔ Mesh; Material-ID-Ansicht | 2.2 |
| Pro Mesh sehen, welche Material-Layer darauf wirken | 5.1 |
| Stylized-Shading mit Ink-Outline im Viewport | 4 |
| Ink-Kanal | 4 |
| **Vektor-Layer:** Bézier-Pfade, die auf der Oberfläche verankert und jederzeit editierbar sind; SVG-Decals | eigener Meilenstein |
| Läuft mit Mesa-Treibern, kostenlos, offenes Projektformat | Leitlinien |
| Export über die Kommandozeile | 13 |

---

## Bewusst nicht übernommen

| Was | Warum | Stattdessen |
|---|---|---|
| `.sbsar`-Materialien und -Filter | Nur mit Adobes proprietärer Substance-Engine ausführbar | eigene Generatoren und Filter als WGSL-Shader |
| Mitgelieferte Adobe-Materialien, Smart Materials, Pinsel | urheberrechtlich geschützt | eigene und CC0-Inhalte |
| Substance 3D Assets (Online-Bibliothek) | Adobe-Dienst mit Konto | Import aus Poly Haven / ambientCG |
| NVIDIA Iray | proprietär und nur für NVIDIA | später ggf. eigener Path Tracer |
| Name, Logo, Icons, Optik | Marken- und Urheberrecht | eigenes Design, gleiche Bedienlogik |

---

## Architektur-Entscheidungen

Diese Punkte müssen **vor** der Brush-Engine feststehen, weil später alles darauf aufbaut.

1. **Striche speichern statt nur Pixel.** Painter speichert jeden Pinselstrich als Daten:
   Punkte auf der Oberfläche, Druck, Pinsel-Einstellungen. Daraus berechnet es die Pixel. Nur
   so geht die Auflösung jederzeit verlustfrei zu ändern, und die Bemalung übersteht einen
   Mesh-Re-Import. **Wir machen es genauso.** Pixel werden zusätzlich gecacht, damit nichts
   neu berechnet werden muss, solange sich nichts ändert. Dazu kommt eine Option „Layer
   rastern“ für sehr schwere Layer. *Seit Phase 3:* Ein Strich speichert seine Tupfer im
   Weltraum (Striche aus der 2D-Ansicht im UV-Raum), die Pinsel-Einstellungen und die Kamera.
   Der Paint Layer auf der GPU ist der Pixel-Cache; geändert wird nur, was neu ist.
2. **Szene statt Einzel-Mesh.** Das Datenmodell bekommt Meshes (mit Namen) und Texture Sets
   (mit Material-Slot und Liste der UV-Tiles). Ein Texture Set besitzt Layer Stack, Kanäle und
   Auflösung.
3. **Layer Stack auf der GPU.** Das Zusammenrechnen der Layer läuft in Shadern, intern
   mit 16-Bit-Float pro Kanal. Neu berechnet wird nur, was sich geändert hat. *Seit Phase 4:*
   Gemalte Pixel liegen in Kacheln (128 × 128), nur wo gemalt wurde. Der Stack wird in
   Blöcken von 512 × 512 Texeln zusammengerechnet; beim Malen nur der bemalte Bereich, nach
   anderen Änderungen die ganze Textur über mehrere Frames verteilt.
4. **Tangent Space nach MikkTSpace**, Normal-Map-Format (OpenGL / DirectX) pro Projekt;
   umgerechnet wird beim Export. *Seit Phase 5:* Intern ist alles OpenGL. Das Format des
   Projekts gilt, wie in Painter, für die Normal Map der Document Channels (später auch
   fürs Baking); die Vorlagen der Engines legen ihr Format selbst fest, und Normal Maps in
   Fill Layern sagen ihres selbst.
5. **Generatoren und Filter als Shader-Dateien:** WGSL-Code plus eine Beschreibung der Parameter
   (Name, Typ, Bereich, Standardwert). Daraus entsteht automatisch die Oberfläche. Das ist
   unser Ersatz für `.sbsar` und gleichzeitig die Plugin-Schnittstelle.
6. **Projektdatei** als ZIP-Container mit JSON (Struktur, Strokes, Einstellungen) und
   Binärdaten (Mesh, gerasterte Caches als EXR). Das Format wird dokumentiert. *Seit
   Phase 5:* `project.json`, `mesh.bin` (das verarbeitete Mesh, wie Painter), `dabs.bin`
   (die Tupfer aller Striche) und die Bilder verlustfrei als PNG bzw. EXR; beschrieben in
   [project-format.md](project-format.md). Striche bleiben Daten und werden beim Öffnen
   neu gemalt; gerasterte Caches kommen erst, wenn das zu lange dauert.
7. **Undo** über Befehle (Command Pattern), die auf den Stroke-Daten arbeiten. Undo braucht so
   nur wenig Speicher. *Seit Phase 3* für Striche umgesetzt, *seit Phase 4* für den Layer
   Stack: Ein Schritt merkt sich den Stack davor; die Striche darin teilen sich beide Stände.

---

## Reihenfolge

Jede Phase endet mit etwas Benutzbarem. **Nach Phase 5 lässt sich ein Spiel-Asset komplett
texturieren und exportieren.**

| Phase | Inhalt | Status |
|---|---|---|
| 1 | **Viewport:** Import (FBX, glTF, OBJ), PBR- und Stylized-Shading, Kamera | ✅ fertig |
| 2 | **Fundament:** Texture Sets und Meshes beim Import, Texture-Set-Liste mit Sichtbarkeit pro Set und pro Mesh, Material-ID-Ansicht, Tangenten, Kanal-Texturen im Shader (inkl. Emissive und Opacity), HDRI-Beleuchtung, Painter-Navigation | ✅ fertig |
| 3 | **Brush-Engine:** Paint und Eraser, Strokes als Daten, Stiftdruck, Alphas, Lazy Mouse, gerade Linien, Undo, 2D-Ansicht | ✅ fertig |
| 4 | **Layer Stack:** Paint, Fill, Ordner, Blend Modes und Deckkraft pro Kanal, Masken, UV- und Tri-planar-Projektion, Kanal-Ansicht | ✅ fertig |
| 5 | **Projekt und Export:** Projektdatei, Export mit Vorlagen für Blender, Unity, Godot, glTF | ✅ fertig |
| 6 | **Oberfläche:** INKRABBIT-Design (Farben, Schriften, eigene Icons, App-Icon), andockbare Fenster und Standard-Layout wie in Painter, gespeicherte Layouts; die Oberfläche bleibt zunächst englisch | ✅ fertig |
| 7 | **Pinsel und Shelf:** Pinsel-Presets, Jitter, Alignment, Stencil, Polygon Fill; Shelf mit Kategorien, eigenen Ressourcen (Pinsel, Alphas, Materialien, Texturen) und PBR-Ordner-Import, Online-Bibliotheken (ambientCG, Poly Haven) | ✅ fertig |
| 8 | **Baking:** AO, Curvature, Position, Thickness, World Normal vom Low-Poly; Normal, Height, ID vom High-Poly | ✅ fertig |
| 9 | **Generatoren:** Mask Builder, Edge Wear, Dirt, Filter, Noises, Smart Materials und Smart Masks, Starter-Inhalte | ✅ fertig (Blur in Masken; weitere Filter folgen) |
| 10 | **Restliche Werkzeuge:** Projection, Symmetrie, Color Selection, Geometry Mask, Anchor Points | läuft: Symmetrie und Color Selection fertig |
| 11 | **Stylized und Vektor-Layer:** Ink-Kanal, Toon-Optionen, Vektor-Pfade auf der Oberfläche | |
| 12 | **Feinschliff:** deutsche und weitere Sprachen, UDIM, Smudge und Clone, Pfad-Werkzeuge, Post-Effekte, Physik-Pinsel (Fur), Python-API, Installer (AppImage / Flatpak, Windows) | |
