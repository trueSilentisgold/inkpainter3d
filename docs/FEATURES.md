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

- [ ] `P0` **Neues Projekt** (Strg+N): Dialog mit Mesh-Datei, Dokument-Auflösung,
      Normal-Map-Format (OpenGL / DirectX) und Template.
- [ ] `P0` **Projekt speichern und öffnen** (Strg+S, Strg+O) in einem eigenen Format: eine
      Datei, die Mesh, Layer, Strokes, Texturen und benutzte Ressourcen enthält.
      Projekte sollen sich auch ohne die Originaldateien öffnen lassen.
- [ ] `P0` Zuletzt geöffnete Projekte, Projekt schließen (Strg+F4).
- [ ] `P1` **Autosave** und Wiederherstellung nach einem Absturz.
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

- [ ] `P0` Beim Import bleiben **Materialzuweisung und Mesh-Namen erhalten**: aus FBX
      (Material pro Polygon), glTF (Material pro Primitive) und OBJ (`usemtl`).
      *Heute fasst der Import alles zu einem Mesh zusammen und verwirft die Materialien.*
- [ ] `P0` Jedes Material wird ein Texture Set. Meshes ohne Material landen in einem
      Standard-Set.
- [ ] `P0` **Tangenten nach MikkTSpace**, damit Normal Maps in Unreal, Unity, Godot und Blender
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

- [ ] `P0` **Ein Auge pro Texture Set** blendet das Set im Viewport ein und aus.
- [ ] `P0` Menü mit **Alle zeigen**, **Alle ausblenden** und **Sichtbarkeit umkehren**.
- [ ] `P0` **Fokus-Modus** (Alt+Q): Nur das aktive Set ist sichtbar, ein zweiter Klick hebt
      das wieder auf.
- [ ] `P0` **Auswahl:** Genau ein Set ist aktiv. Layer Stack, Texture Set Settings und Shader
      Settings zeigen immer das aktive Set.
- [ ] `P0` **Set im Viewport wählen:** Strg+Alt+Rechtsklick auf das Modell aktiviert das Set
      unter dem Mauszeiger.
- [ ] `P1` **Umbenennen** per Doppelklick (der Name wird beim Export verwendet), auf den
      Originalnamen zurücksetzen, eine Beschreibung pro Set.
- [ ] `P1` **Shader-Instanz pro Set:** Standardmäßig teilen sich alle Sets einen Shader; per
      „Neue Shader-Instanz“ bekommt ein Set einen eigenen.
- [ ] `P1` Einstellung „Beim Malen nur das aktive Set anzeigen“ (in Painter standardmäßig an,
      spart Leistung).

Über Painter hinaus (**＋**). Painter kann einzelne Meshes innerhalb eines Texture Sets nicht
ausblenden; bei Adobe liegt dazu ein offener Feature-Wunsch.

- [ ] `P0` ＋ **Jedes Set lässt sich aufklappen** und zeigt die Meshes, die dieses Material
      benutzen, mit Dreiecksanzahl.
- [ ] `P0` ＋ **Ein Auge pro Mesh** blendet einzelne Meshes aus, auch innerhalb eines Sets.
      Ausgeblendete Meshes werden nicht bemalt, sodass man darunter malen kann.
- [ ] `P0` ＋ **Hervorheben beim Überfahren:** Fährt man in der Liste über ein Set oder Mesh,
      leuchtet es im Viewport auf.
- [ ] `P1` ＋ **Umgekehrte Ansicht „Nach Mesh“:** Mesh → seine Materialien. Meshes mit mehreren
      Materialien bekommen ein Symbol (⧉).
- [ ] `P1` ＋ **Material-ID-Ansicht im Viewport:** Jedes Texture Set erscheint in einer eigenen
      Farbe, damit man auf einen Blick sieht, was wozu gehört.
- [ ] `P1` ＋ **Info unter dem Mauszeiger:** In der Statusleiste stehen Mesh-Name, Texture Set und
      UV-Tile unter dem Cursor.
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

- [ ] `P0` 3D-Ansicht
- [ ] `P1` **2D-Ansicht** (UV-Layout), in der man ebenfalls malen kann
- [ ] `P1` Layouts **F1** 3D+2D, **F2** nur 3D, **F3** nur 2D, **F4** 3D und 2D tauschen
- [ ] `P1` Kamera **perspektivisch (F5)** oder **orthografisch (F6)**, Brennweite / FOV
- [ ] `P1` UI ausblenden (Tab)

**Beleuchtung**

- [ ] `P0` **HDRI-Umgebung (Image Based Lighting).** Ersetzt die jetzige Platzhalter-Beleuchtung.
- [ ] `P0` **Umgebung drehen mit Shift+Rechts ziehen**, dazu Belichtung,
      Hintergrund-Deckkraft und Hintergrund-Unschärfe.
- [ ] `P0` Mitgelieferte HDRIs (CC0, z. B. von Poly Haven)
- [ ] `P1` Schatten
- [ ] `P1` Viewport-Ambient-Occlusion

**Anzeige-Modi**

- [ ] `P0` **Material (M):** fertiges Material
- [ ] `P0` **Kanal (C / Shift+C):** einzelnen Kanal durchschalten, z. B. nur Roughness
- [ ] `P1` **Mesh Map (B / Shift+B):** gebackene Maps durchschalten
- [ ] `P1` Maske eines Layers anzeigen (Alt+Klick auf die Maske)
- [ ] `P1` ＋ Wireframe und UV-Nähte als Overlay

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
| `P0` | Base Color, Roughness, Metallic, Normal, Height |
| `P1` | Ambient Occlusion, Emissive, Opacity, Specular / Glossiness (für Spec-Gloss), User-Kanäle (frei benennbar, mehrere) |
| `P2` | Specular Level, Displacement, Anisotropy, Transmission, IOR, Scattering (SSS), Sheen, Coat |
| **＋** | **Ink** (Linienfarbe und -stärke für den Stylized-Look) |

Die Farbbehandlung richtet sich nach der Rolle des Kanals: Base Color ist sRGB, Roughness ist
lineare Daten. Bei User-Kanälen ist sie umschaltbar.

**Shader**

- [ ] `P0` **PBR Metallic/Roughness** (GGX)
- [x] `P0` ＋ **Stylized / Toon** mit Ink-Outline. Das Grundgerüst steht, wird aber erweitert.
- [ ] `P1` **PBR Specular/Glossiness**
- [ ] `P1` Opacity: Alpha Test und Alpha Blend
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

- [ ] `P0` **Ohne Maske** deckt ein Material das ganze Texture Set ab. Alles darunter ist
      verdeckt.
- [ ] `P0` **Mit Maske:** Weiß zeigt dieses Material, Schwarz lässt die Materialien darunter
      durch, Grau mischt beide. Graustufen ergeben weiche Übergänge, z. B. abgeplatzten Lack
      an Kanten.
- [ ] `P0` **Priorität ändern** durch Verschieben im Stack per Drag & Drop.
- [ ] `P0` **Pro Kanal:** Ein Material gewinnt nur in den Kanälen, die es eingeschaltet hat.
      Ein „Nass“-Layer, der nur Roughness setzt, lässt die Farbe darunter stehen.
- [ ] `P0` **Deckkraft und Blend Mode** verändern das Ergebnis: Bei *Normal* mit 100 % gilt
      „oben gewinnt“, bei weniger Deckkraft oder anderen Modi wird gemischt.
- [ ] `P0` **Ordner mit Maske:** Mehrere Layer lassen sich gruppieren und gemeinsam maskieren,
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

- [ ] `P0` **Paint Layer**
- [ ] `P0` **Fill Layer**
- [ ] `P0` **Ordner (Gruppe)**
- [ ] `P2` **Instanz-Layer:** ein Layer, der in mehreren Texture Sets gleichzeitig wirkt
- [ ] `P2` **Layer zusammenführen (Flatten)**, auch als Export auf die Festplatte

**Eigenschaften pro Layer**

- [ ] `P0` **Blend Mode und Deckkraft pro Kanal.** Wie in Painter wählt ein Dropdown oben im
      Layer Stack den Kanal, dessen Blending man gerade bearbeitet.
- [ ] `P0` Kanäle pro Layer ein- und ausschalten (z. B. Layer wirkt nur auf Roughness)
- [ ] `P0` Sichtbarkeit, Umbenennen, Reihenfolge per Drag & Drop
- [ ] `P1` Vorschaubilder für Layer und Masken

**Blend Modes**

- [ ] `P0` Normal, Passthrough, Disable, Replace, Multiply, Screen, Overlay,
      Linear Dodge (Add), Subtract, Normal Map Combine, Normal Map Detail
- [ ] `P1` die übrigen Modi aus Painter: Divide, Inverse Divide, Darken (Min), Lighten (Max),
      Inverse Subtract, Difference, Exclusion, Signed Addition, Linear Burn, Color Burn,
      Color Dodge, Soft Light, Hard Light, Vivid Light, Linear Light, Pin Light, Tint,
      Saturation, Color, Value, Normal Map Inverse Detail

**Bearbeiten**

- [ ] `P0` Kopieren (Strg+C), Ausschneiden (Strg+X), Einfügen (Strg+V), Löschen (Entf),
      Duplizieren (Strg+D), Gruppieren (Strg+G)
- [ ] `P1` Layer zwischen Texture Sets kopieren
- [ ] `P1` Nur den Layer-Inhalt kopieren und einfügen (Strg+Shift+C / V)
- [ ] `P2` Material Layering (Unter-Stacks pro Texture Set)

---

## 6. Masken und Effekte

**Masken**

- [ ] `P0` **Schwarze oder weiße Maske** hinzufügen und hineinmalen
- [ ] `P1` Maske ein- und ausschalten (Shift+Klick), Maske im Viewport anzeigen (Alt+Klick)
- [ ] `P1` **Color Selection:** Maske aus Farben der ID-Map
- [ ] `P1` **Smart Masks:** gespeicherte Masken-Aufbauten aus dem Shelf
- [ ] `P2` **Quick Mask** (U bearbeiten, Y leeren, I invertieren)

**Effekte.** Sie liegen in der Maske und auch im Inhalt eines Layers und bilden dort einen
eigenen Stapel:

- [ ] `P0` **Paint**
- [ ] `P0` **Fill**
- [ ] `P1` **Levels**
- [ ] `P1` **Filter** (siehe Kapitel 11)
- [ ] `P1` **Generator** (siehe Kapitel 11)
- [ ] `P2` **Compare Mask**
- [ ] `P2` **Anchor Point:** Zwischenergebnis des Stacks, das spätere Generatoren und Filter
      als Eingabe nutzen. Klassisches Beispiel: Kanten-Abnutzung, die gemalte Height-Details
      berücksichtigt.

---

## 7. Werkzeuge

Taste in Klammern = Painter-Shortcut.

**Malen und Füllen**

- [ ] `P0` **Paint (1)**
- [ ] `P0` **Eraser (2)**
- [ ] `P1` **Projection (3):** malt mit einer projizierten Textur, die sich mit W / E / R
      verschieben, drehen und skalieren lässt
- [ ] `P1` **Polygon Fill (4):** füllt nach Dreieck, Polygon, Mesh oder UV-Insel. Ideal für
      Masken.
- [ ] `P1` **Pick Material (P):** übernimmt alle Kanalwerte vom Modell
- [ ] `P2` **Smudge (5)**
- [ ] `P2` **Clone (6 relativ, Strg+6 absolut):** Quelle mit V+Klick setzen

**Strich-Hilfen**

- [ ] `P0` **Gerade Linien:** Shift+Klick, mit Strg+Shift+Klick auf Winkel eingerastet
- [ ] `P0` **Lazy Mouse (D):** Strichglättung
- [ ] `P1` **Symmetrie (L):** Spiegeln an X / Y / Z, lokal oder Welt; radiale Symmetrie
- [ ] `P1` **Vorherigen Strich fortsetzen (A)**
- [ ] `P1` **Stencil:** Maske über dem Viewport; S+Links drehen, S+Mitte verschieben,
      S+Rechts zoomen, N ignoriert die Stencil-Maske
- [ ] `P1` **Graustufen invertieren (X)**

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

- [ ] `P0` **Size:** Strg+Rechts horizontal ziehen oder [ und ]. Der Größenraum ist wählbar
      (Objekt, Welt, Bildschirm).
- [ ] `P0` **Hardness:** Strg+Rechts vertikal ziehen
- [ ] `P0` **Flow:** Strg+Links horizontal ziehen
- [ ] `P0` **Stroke Opacity**
- [ ] `P0` **Spacing**
- [ ] `P1` **Angle:** Strg+Links vertikal ziehen. Option „Follow Path“.
- [ ] `P1` **Jitter:** Size, Flow, Angle, Position

**Projektion**

- [ ] `P0` **Backface Culling** mit Winkel
- [ ] `P1` **Alignment:** Tangent Wrap, Tangent Planar, Camera, UV

**Tablet**

- [ ] `P0` **Stiftdruck**, pro Parameter zuweisbar (Size, Flow, Opacity …)
- [ ] `P1` **Stift-Neigung (Tilt)**

**Spitze und Material**

- [ ] `P0` **Alpha:** Pinselform aus einem Graustufenbild
- [ ] `P1` **Stencil**
- [ ] `P0` **Material des Pinsels:** Wert oder Farbe pro Kanal; Kanäle einzeln ein- und
      ausschaltbar
- [ ] `P1` Material des Pinsels aus Textur oder Material aus dem Shelf

**Presets**

- [ ] `P1` **Pinsel-Presets** speichern und laden

---

## 9. Fill Layer und Projektionen

**Projektionsarten**

- [ ] `P0` **UV-Projektion:** Kachelung, Versatz, Rotation
- [ ] `P0` **Tri-planar:** nahtlos über UV-Nähte, mit einstellbarer Kantenhärte
- [ ] `P1` **Planar**
- [ ] `P1` **Sphärisch**
- [ ] `P1` **Zylindrisch**
- [ ] `P1` **Fill (match per UV Tile)**
- [ ] `P2` **Warp-Projektion:** Projektionsbox mit verschiebbaren Ecken, mit Automatik-Modus
- [ ] `P2` **UV-Set zu UV-Set**

**Füllinhalt**

- [ ] `P0` Fester Wert oder Farbe pro Kanal
- [ ] `P0` Bild-Textur pro Kanal
- [ ] `P1` Material aus dem Shelf, prozedurale Texturen
- [ ] `P1` Material per Drag & Drop direkt auf den Layer Stack oder das Modell ziehen
      (Strg+Drag wirkt nur auf das Teil unter dem Mauszeiger)

---

## 10. Baking (Mesh Maps)

- [ ] `P1` **Baking-Fenster** (Strg+Shift+B, Baking-Modus F8)

**Maps**

| Prio | Maps | Hinweis |
|---|---|---|
| `P1` | Ambient Occlusion, Curvature, World Space Normal, Position, Thickness | vom Low-Poly-Mesh selbst. Die Generatoren brauchen sie. |
| `P1` | Normal (Tangent Space), Height, ID | von High-Poly auf Low-Poly; ID aus Vertex-Farbe, Material-Farbe oder Mesh-ID |
| `P2` | Bent Normals, Opacity | |

**Einstellungen**

- [ ] `P1` Ausgabegröße, Dilation, Antialiasing (Subsampling)
- [ ] `P1` High-Poly-Dateien (mehrere)
- [ ] `P1` Zuordnung *Immer* oder *nach Mesh-Name* (Suffix `_low` / `_high`)
- [ ] `P1` Max. Frontal- und Rear-Distanz (auch relativ zur Bounding Box), Cage-Datei,
      gemittelte Normalen, Backfaces ignorieren
- [ ] `P1` Auswahl, welche Texture Sets gebacken werden
- [ ] `P1` Einstellungen pro Map, z. B. AO-Strahlenzahl und Verteilung, Self-Occlusion,
      Ground Plane

**Technik**

- [ ] `P1` GPU-Raytracing über eine eigene BVH im Compute-Shader. Das läuft auf jeder
      Vulkan-GPU und braucht keine RT-Kerne.
- [ ] `P2` Automatisch neu backen, wenn sich das Mesh ändert

---

## 11. Generatoren, Filter, Smart Materials

Painter liefert diese Bausteine als `.sbsar`-Dateien aus, die nur Adobes Engine ausführen kann.
**Wir bauen sie selbst:** als WGSL-Shader mit einer Parameter-Beschreibung (siehe
[Architektur-Entscheidungen](#architektur-entscheidungen)).

**Generatoren** (nutzen die Mesh Maps)

- [ ] `P1` **Mask Builder:** kombiniert Curvature, AO, Position und Thickness mit Rauschen; der
      wichtigste Generator überhaupt
- [ ] `P1` **Edge Wear:** Kanten-Abnutzung
- [ ] `P1` **Dirt:** Schmutz in Vertiefungen
- [ ] `P1` **Position Gradient:** Verlauf, z. B. von unten nach oben
- [ ] `P1` **Light:** gerichtete Beleuchtung, auch für den Stylized-Look
- [ ] `P1` Curvature, AO und Thickness einzeln als Generator

**Filter**

- [ ] `P1` Blur, Sharpen, HSL / Farbkorrektur, Gradient Map, Height-zu-Normal
- [ ] `P2` Directional Blur, Slope Blur, Warp, Baked Lighting
- [ ] `P2` ＋ Stylized-Filter: Posterize, Hand-Painted-Look, Pixel Art

**Prozedurale Texturen**

- [ ] `P1` Noises (Perlin, Fractal, Voronoi / Zellen, Wolken)
- [ ] `P1` Grunges (Kratzer, Flecken, Staub)
- [ ] `P1` Muster (Kacheln, Ziegel, Streifen)

**Presets**

- [ ] `P1` **Smart Materials:** Layer-Ordner als Preset speichern; passt sich über die Mesh Maps
      an jedes Modell an
- [ ] `P1` **Smart Masks:** Masken-Aufbau als Preset

**Mitgelieferte Inhalte**

- [ ] `P1` **Starter-Bibliothek:** Metalle, Kunststoffe, Holz, Stoff, Leder, Stein; ein Dutzend
      Smart Materials; Pinsel, Alphas, Grunges, HDRIs. Alles selbst erstellt oder CC0
      (Poly Haven, ambientCG).

---

## 12. Shelf (Asset-Bibliothek)

- [ ] `P1` **Kategorien** wie in Painter: Materials, Smart Materials, Smart Masks, Brushes,
      Alphas, Grunges, Procedurals, Textures, Filters, Generators, Environments, Export
      Presets, Templates
- [ ] `P1` Suche, Filter, Vorschaubilder; Drag & Drop auf Layer Stack, Viewport und
      Parameterfelder
- [ ] `P1` **Eigene Ressourcen importieren** (Bilder, Presets), entweder in die Bibliothek oder
      nur ins Projekt
- [ ] `P2` Eigene Bibliotheks-Ordner (Sub-Libraries), Favoriten, Tags
- [ ] `P2` ＋ **PBR-Materialordner importieren:** ein Ordner mit Base Color, Roughness, Normal …
      im Format von Poly Haven oder ambientCG wird automatisch ein Material

---

## 13. Export

- [ ] `P0` **Export-Dialog** (Strg+Shift+E) mit Zielordner, Auswahl der Texture Sets und
      Vorschau der erzeugten Dateien
- [ ] `P0` **Mitgelieferte Vorlagen:** Unreal Engine (Packed: Occlusion, Roughness, Metallic in
      einer Textur), Unity (URP, HDRP), Godot, Blender, glTF PBR Metal Roughness, Document
      Channels
- [ ] `P0` **Dateiformate** PNG, TGA, JPEG mit 8 Bit; `P1` PNG mit 16 Bit, TIFF, EXR (16 / 32
      Bit Float); `P2` PSD mit Layern, DDS
- [ ] `P0` **Padding:** Dilation unendlich oder N Pixel, Dilation mit transparentem oder
      Standard-Hintergrund, kein Padding
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

> **Achtung:** Die heutige Navigation weicht ab. Rechte Maustaste allein dreht, Shift+Rechts
> verschiebt. Das wird auf das Painter-Schema umgestellt, weil Shift+Rechts dort die Umgebung
> dreht. `P2` ＋: Alternative Navigationsschemata (Blender, Maya) als Option.

---

## 15. Oberfläche

- [ ] `P1` **Andockbare Fenster:** Layer Stack, Texture-Set-Liste, Texture Set Settings,
      Properties (zeigt je nach Auswahl Werkzeug, Layer oder Effekt), Shelf, Display Settings,
      Shader Settings, Viewer Settings, History, Log
- [ ] `P1` Fenster-Layouts speichern und wiederherstellen; Standard-Layout ähnlich wie in
      Painter
- [ ] `P1` Dunkles Design, skalierbar für HiDPI
- [ ] `P2` Übersetzungen (Deutsch, Englisch)

---

## 16. Undo, Leistung, Stabilität

- [ ] `P0` **Undo / Redo** für alle Aktionen (Striche, Layer, Einstellungen)
- [ ] `P1` **History-Fenster**
- [ ] `P0` **Flüssiges Malen bei 4K pro Texture Set** auf einer Mittelklasse-GPU
- [ ] `P1` Mehrere 4K-Sets gleichzeitig im Projekt
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
   rastern“ für sehr schwere Layer.
2. **Szene statt Einzel-Mesh.** Das Datenmodell bekommt Meshes (mit Namen) und Texture Sets
   (mit Material-Slot und Liste der UV-Tiles). Ein Texture Set besitzt Layer Stack, Kanäle und
   Auflösung.
3. **Layer Stack auf der GPU.** Das Zusammenrechnen der Layer läuft in Compute-Shadern, intern
   mit 16-Bit-Float pro Kanal. Neu berechnet wird nur, was sich geändert hat.
4. **Tangent Space nach MikkTSpace**, Normal-Map-Format (OpenGL / DirectX) pro Projekt;
   umgerechnet wird beim Export.
5. **Generatoren und Filter als Shader-Dateien:** WGSL-Code plus eine Beschreibung der Parameter
   (Name, Typ, Bereich, Standardwert). Daraus entsteht automatisch die Oberfläche. Das ist
   unser Ersatz für `.sbsar` und gleichzeitig die Plugin-Schnittstelle.
6. **Projektdatei** als ZIP-Container mit JSON (Struktur, Strokes, Einstellungen) und
   Binärdaten (Mesh, gerasterte Caches als EXR). Das Format wird dokumentiert.
7. **Undo** über Befehle (Command Pattern), die auf den Stroke-Daten arbeiten. Undo braucht so
   nur wenig Speicher.

---

## Reihenfolge

Jede Phase endet mit etwas Benutzbarem. **Nach Phase 5 lässt sich ein Spiel-Asset komplett
texturieren und exportieren.**

| Phase | Inhalt | Status |
|---|---|---|
| 1 | **Viewport:** Import (FBX, glTF, OBJ), PBR- und Stylized-Shading, Kamera | ✅ fertig |
| 2 | **Fundament:** Texture Sets und Meshes beim Import, Texture-Set-Liste mit Sichtbarkeit pro Set und pro Mesh, Material-ID-Ansicht, Tangenten, Kanal-Texturen im Shader, HDRI-Beleuchtung, Painter-Navigation | ⏳ als Nächstes |
| 3 | **Brush-Engine:** Paint und Eraser, Strokes als Daten, Stiftdruck, Alphas, Lazy Mouse, gerade Linien, Undo | |
| 4 | **Layer Stack:** Paint, Fill, Ordner, Blend Modes und Deckkraft pro Kanal, Masken, UV- und Tri-planar-Projektion, Kanal-Ansicht | |
| 5 | **Projekt und Export:** Projektdatei, Export mit Vorlagen für Unreal, Unity, Godot, Blender, glTF | |
| 6 | **Baking:** AO, Curvature, Position, Thickness, World Normal vom Low-Poly; Normal, Height, ID vom High-Poly | |
| 7 | **Generatoren und Bibliothek:** Mask Builder, Edge Wear, Dirt, Filter, Noises, Smart Materials und Smart Masks, Shelf, Starter-Inhalte | |
| 8 | **Restliche Werkzeuge:** Projection, Polygon Fill, Symmetrie, Stencil, Color Selection, Geometry Mask, Anchor Points, 2D-Ansicht | |
| 9 | **Stylized und Vektor-Layer:** Ink-Kanal, Toon-Optionen, Vektor-Pfade auf der Oberfläche | |
| 10 | **Feinschliff:** UDIM, Smudge und Clone, Pfad-Werkzeuge, Post-Effekte, Python-API, Installer (AppImage / Flatpak, Windows) | |
