# Horizon Voice Chat

Proximity-Sprachchat für [Horizon](https://github.com/horizon-rust/horizon) (Minecraft Java 26.3 in Rust):
Du hörst die Spieler in deiner Nähe, leiser mit der Entfernung und aus der Richtung, in der sie stehen.

Der Mod spricht das Netzwerkprotokoll von
[Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat) (Kompatibilitätsversion 21, also
Simple Voice Chat 2.6.x / 2.7.x für Minecraft 26.3). Dadurch können auf demselben Server
Horizon-Spieler mit diesem Mod und Java-Spieler mit dem originalen Simple Voice Chat miteinander
reden.

> **Stand: 0.2.0, Sprache läuft im Spiel; Menüs und Gruppen sind noch ungetestet im Spiel.** Siehe [Testen](#testen).

## Welche Horizon-Version

Die Mods brauchen Horizon ab der Nightly vom 22.11.2026 (`f1bb66f`), der ersten mit Mod-SDK
(`horizon-mod.toml`, `horizon-mod-api`). Die Fassung für die ältere API (Nightly vom 13.11.2026,
`horizon-mixin.toml`) liegt nicht mehr auf `main`, sondern im Branch
[`legacy`](https://github.com/cellow02/horizon-voice-chat/tree/legacy/legacy). Sie hat kein
Sprachchat-Menü und wird nicht weiterentwickelt.

## Die zwei Mods

| Ordner | Mod-ID | Seite | Aufgabe |
| --- | --- | --- | --- |
| [`client/`](client) | `horizon_voice` | `client` | Mikrofon, Wiedergabe, Tasten, HUD. Blockiert nie den Beitritt zu einem Server. |
| [`server/`](server) | `horizon_voice_server` | `server` | Vergibt die Schlüssel, betreibt den Sprachserver (UDP) und verteilt die Sprache an Spieler in Reichweite. |

Es sind zwei getrennte Mods, weil Horizon bei einem `side = "both"`-Mod jeden Client ohne diesen Mod
ablehnt, auch Java-Clients. Mit einem reinen Server-Mod bleibt der Server für alle offen.

| Du spielst … | Du brauchst |
| --- | --- |
| auf einem Horizon-Server | `client/` bei dir, `server/` auf dem Server |
| als LAN-Host oder im Einzelspieler | beide Mods in deiner Instanz |
| mit Horizon auf einem Paper-/Fabric-Server mit Simple Voice Chat | nur `client/` |
| mit Java-Minecraft auf einem Horizon-Server | das originale Simple Voice Chat bei dir, `server/` auf dem Server |

## Installieren

Beides sind Native-Mods: Der Horizon-Launcher kompiliert den Rust-Quelltext auf deinem Rechner. Der
erste Bau lädt eine Toolchain und dauert laut Horizon 20 bis 30 Minuten. Native-Mods laufen ohne
Sandbox mit deinen Rechten, der Launcher fragt deshalb einmal nach deiner Zustimmung.

**Client:** Im Launcher auf der Mods-Seite **Add from GitHub** mit der Adresse dieses Repos, oder:

```sh
horizon-launcher mods add-github https://github.com/cellow02/horizon-voice-chat --mod horizon_voice --trust-native-code
```

Für einen LAN-Host oder den Einzelspieler statt `--mod horizon_voice` einfach `--all`, das
installiert auch den Server-Mod. Danach die Instanz bauen lassen (`horizon-launcher native-build`).

**Dedizierter Server ("Server-Kompilation"):**

```sh
git clone https://github.com/cellow02/horizon-voice-chat
horizon-launcher native-build horizon-voice-chat/server --server-output pfad/zum/export --version <Horizon-Version> --trust-native-code
horizon-server --dir SERVER config set engine.path=/absoluter/pfad/zum/export/horizon-engine
horizon-server --dir SERVER restart
```

Danach den **UDP-Port 24454** in Firewall und Router freigeben, so wie den Spielport.

Die Mods sind auf `f1bb66f` vom 22.11.2026 gepinnt (`[native]` in `horizon-mod.toml`), ihre
`Cargo.lock` auf die Crate-Versionen genau dieses Spiels. Der Launcher baut einen Mod nur gegen den
gepinnten Stand; für einen anderen siehe [Entwickeln](#entwickeln).

Linux: Der Client spielt und nimmt über ALSA auf. Er lädt dafür zur Laufzeit dieselbe
`libasound.so.2`, die das Spiel schon benutzt, und kompiliert selbst keine ALSA-Anbindung: Der Launcher
lehnt einen Mod ab, der eine zweite Kopie einer nativen Bibliothek des Spiels mitbringen würde. Auf
Windows und macOS nimmt der Client cpal. Der Server-Mod hat keine Audio-Abhängigkeit.

## Bedienen

| Taste (Standard) | Wirkung |
| --- | --- |
| `V` | Sprachchat-Menü |
| `G` | Gruppen-Menü |
| `Feststelltaste` | Push-to-Talk: sprechen, solange gehalten |
| nicht belegt | Flüstern: beim Sprechen halten, trägt nur halb so weit |
| `M` | Mikrofon stumm |
| `N` | Sprachchat ganz aus (du hörst und sendest nichts) |
| `H` | Sprachchat-Symbole ausblenden |

Die Tasten stehen unter **Optionen > Steuerung > Sprachchat**. Die Menüs sind wie bei Simple Voice
Chat aufgebaut, mit eigenen Symbolen:

- **Sprachchat (`V`):** Einstellungen, Gruppe, und unten Mikrofon stumm, Sprachchat aus,
  Lautstärke pro Spieler und Symbole ausblenden.
- **Einstellungen:** Sprachlautstärke, Mikrofonverstärkung, Push-to-Talk oder Sprachaktivierung mit
  Schwellwert (der Knopf mit dem Mikrofon spielt dein Mikrofon zum Testen ab), Stereo oder Mono,
  Mikrofon und Lautsprecher wählen.
- **Lautstärken:** jeder Spieler einzeln, 0 bis 400 %.
- **Gruppen (`G`):** Gruppen beitreten (mit Passwort, falls gesetzt) oder eine erstellen: Normal
  (nur die Gruppe hört dich, du hörst auch die Umgebung), Offen (die Umgebung hört dich auch) oder
  Isoliert (du hörst nur die Gruppe). In der Gruppe zeigt das Menü die Mitglieder, wer spricht, und
  Knöpfe für stumm, aus, Gruppenanzeige und Verlassen.

Unten links zeigt das HUD, ob du sprichst, stumm oder aus bist, und wer in der Nähe spricht; oben
links stehen die Mitglieder deiner Gruppe. Die Einstellungen landen in
`config/horizon_voice-client.properties`.

Weil die Mod-API keine Spielerskins hergibt, sind die Köpfe in den Menüs einfache Gesichter in
Farben, die zum Spieler gehören. Symbole über den Namensschildern gehen mit der API auch nicht.

**Du hörst dich selbst doppelt?** Der Server schickt niemandem die eigene Stimme zurück, und der
Client verwirft sie jetzt zusätzlich. Bleibt das Echo, kommt es fast immer von außen: zwei Clients
auf einem PC mit demselben Mikrofon, oder ein Mitspieler ohne Kopfhörer, dessen Mikrofon deine
Stimme aus seinen Lautsprechern wieder aufnimmt.

## Server einstellen

Der Server legt `config/horizon_voice_server-server.toml` an (mit der neuen API pro Welt unter
`serverconfig/` überschreibbar):

| Schlüssel | Standard | Bedeutung |
| --- | --- | --- |
| `port` | `24454` | UDP-Port des Sprachservers |
| `bind_address` | leer | IP, an die gebunden wird; leer heißt alle IPv4-Adressen |
| `voice_host` | leer | Was Clients für Sprache ansteuern, wenn es von der Spieladresse abweicht: ein Port oder `host[:port]` |
| `distance` | `48.0` | Reichweite normaler Sprache in Blöcken |
| `whisper_distance` | `24.0` | Reichweite beim Flüstern |
| `keep_alive` | `1000` | Millisekunden zwischen Keep-Alive-Paketen |
| `mtu_size` | `1024` | Größtes Sprachpaket in Bytes |
| `allow_recording` | `true` | Ob Clients aufnehmen dürfen |
| `groups` | `true` | Ob Spieler Gruppen erstellen und ihnen beitreten können |

Port, Reichweiten und Keep-Alive gelten nach einem Neustart des Servers.

## Was (noch) fehlt

- Dauerhafte Gruppen aus der Server-Konfiguration, Lautstärke-Kategorien, Aufnahme,
  Rauschunterdrückung und die Plugin-API von Simple Voice Chat.
- Echte Spielerköpfe und Symbole an den Namensschildern (die Mod-API kennt keine Skins).
- **Zuschauer-Sonderregeln** und vom Server erzwungenes Stummschalten.
- **SRV-Einträge:** Der Client schickt Sprache an den Hostnamen, mit dem du beigetreten bist. Zeigt
  ein SRV-Eintrag auf einen anderen Rechner, muss der Server `voice_host` setzen.
- **Ältere Protokollversionen:** Der Server nimmt Clients mit Version 21 und Version 20 (Simple Voice
  Chat 2.6.x) an. Der Client spricht nur Version 21, braucht also einen Server mit Version 21. Bei
  Clients mit Version 20 sehen andere nicht, ob sie den Sprachchat ausgeschaltet haben.

## Testen

Was automatisch geprüft ist:

```sh
cargo test --manifest-path interop-tests/Cargo.toml   # Protokoll, Client- und Server-Netzcode über Loopback,
                                                        # und die Gruppen
cd client && cargo test                                 # zusätzlich Mischer, Mikrofon-Logik und Einstellungen
cd server && cargo test                                 # Konfiguration und Gruppen
```

Was im Spiel noch geprüft werden muss:

1. Baut der Launcher die Mods (`native-build`), und lädt Horizon sie ohne Ablehnung?
2. Zwei Horizon-Clients auf einem Horizon-Server: Verbinden, hören, Richtung, Entfernung, Dimension.
   Menüs (`V`, `G`), Gruppen aller drei Arten, Geräteauswahl, Lautstärke pro Spieler.
3. Ein Java-Client mit Simple Voice Chat auf demselben Server: Hören beide Seiten einander?
4. Ein Horizon-Client auf einem Paper-Server mit Simple Voice Chat.
5. LAN-Host mit beiden Mods.

Im Log stehen die Zeilen des Mods unter `Voice chat …`.

## Entwickeln

```
shared/protocol/   Das Protokoll (Quelle). tools/sync-protocol.sh kopiert es in beide Mods,
                   weil ein Native-Mod keine Dateien außerhalb seines Ordners verwenden darf.
client/src/        net.rs (UDP-Verbindung), audio.rs (Mikrofon-Logik), sound_alsa.rs (Geräte unter Linux),
                   sound_cpal.rs (Geräte unter Windows/macOS), codec.rs (Opus), mixer.rs, lib.rs (Hooks)
client/src/        ui.rs (Menüs), icons.rs (eigene Symbole), settings.rs (Einstellungsdatei)
server/src/        udp.rs (Sprachserver), groups.rs (Gruppen), config.rs, lib.rs (Hooks)
interop-tests/     Client- und Server-Netzcode gegeneinander, ohne Spiel
PROTOCOL.md        Beschreibung des Protokolls
```

Nach Änderungen an `shared/protocol/` `tools/sync-protocol.sh` ausführen: Es kopiert das Protokoll
in beide Mods (`--check` prüft nur).

Zum lokalen Kompilieren ohne Launcher erwartet jeder Mod das SDK unter `.horizon/sdk/api`
(`cargo horizon-mod` legt es an; ein Symlink auf einen Checkout von `horizon-rust/horizon` geht
auch). Ein lokales `cargo check` oder `cargo test` im Mod-Ordner schreibt `Cargo.lock` um. Die
eingecheckte `Cargo.lock` ist die Abhängigkeits-Sperre, die der Launcher verlangt: vor dem
Committen mit `git checkout -- Cargo.lock` zurücksetzen.

Der Launcher baut auf dem Client nur die Mods und nimmt das Spiel fertig aus dem SDK. Jede Crate,
die ein Mod mit dem Spiel teilt, muss deshalb in dessen `Cargo.lock` genau die Version und die
Abhängigkeiten aus der `Cargo.lock` des Spiels haben. `horizon-launcher native-lock --update-lock`
löst neuere Versionen auf und taugt dafür nicht. Nach geänderten Abhängigkeiten oder einem neuen
Horizon-Stand die Sperre so neu erzeugen (Horizon-Checkout auf dem gepinnten Commit):

```sh
tools/make-lock.sh client pfad/zu/horizon/Cargo.lock
tools/make-lock.sh server pfad/zu/horizon/Cargo.lock
```

Das Skript prüft das Ergebnis wie der Launcher und bricht ab, wenn eine Crate des Spiels abweicht.

## Herkunft und Lizenz

Dieser Mod ist eine eigenständige Neuentwicklung in Rust unter der [MIT-Lizenz](LICENSE). Er ist
kein Port und kein Fork von Simple Voice Chat und enthält keinen Code daraus. Simple Voice Chat ist
von Max Henkel ([henkelmax](https://github.com/henkelmax)) und steht unter "All Rights Reserved".
Nachgebaut ist nur das Netzwerkformat, damit beide Mods miteinander reden können; es ist in
[PROTOCOL.md](PROTOCOL.md) beschrieben. Dieses Projekt ist nicht mit Simple Voice Chat, Horizon,
Mojang oder Microsoft verbunden und wird von ihnen nicht unterstützt.

Verwendete Bibliotheken: [cpal](https://github.com/RustAudio/cpal) (Apache-2.0),
[unsafe-libopus](https://github.com/DCNick3/unsafe-libopus) (BSD-3-Clause, libopus in Rust),
[aes-gcm](https://github.com/RustCrypto/AEADs), [hmac](https://github.com/RustCrypto/MACs),
[sha2](https://github.com/RustCrypto/hashes) (MIT/Apache-2.0), [getrandom](https://github.com/rust-random/getrandom)
(MIT/Apache-2.0). Die Mods bauen gegen die Horizon-Mod-API (LGPL-3.0-or-later).
