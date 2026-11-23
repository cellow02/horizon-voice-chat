# Horizon Voice Chat

Proximity-Sprachchat für [Horizon](https://github.com/horizon-rust/horizon) (Minecraft Java 26.3 in Rust):
Du hörst die Spieler in deiner Nähe, leiser mit der Entfernung und aus der Richtung, in der sie stehen.

Der Mod spricht das Netzwerkprotokoll von
[Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat) (Kompatibilitätsversion 21, also
Simple Voice Chat 2.6.x / 2.7.x für Minecraft 26.3). Dadurch können auf demselben Server
Horizon-Spieler mit diesem Mod und Java-Spieler mit dem originalen Simple Voice Chat miteinander
reden.

> **Stand: 0.2.0, Sprache läuft im Spiel; Menüs und Gruppen sind noch ungetestet im Spiel.** Alle vier Mods kompilieren gegen ihre Horizon-API, und die
> Netzwerk-, Codec- und Mischer-Tests laufen durch. In einem echten Horizon-Client, auf einem echten
> Horizon-Server und gegen einen echten Simple-Voice-Chat-Client ist der Mod noch nicht gelaufen.
> Siehe [Testen](#testen).

## Welche Horizon-Version

Horizon hat sein Mod-System Mitte November 2026 umgebaut. Der Mod liegt deshalb zweimal im Repo, mit
demselben Netzwerk- und Audio-Code darunter:

| Dein Horizon | Ordner | Mod-API |
| --- | --- | --- |
| Nightly vom 13.11.2026 (`9f42064`), die bisherige API | [`legacy/client`](legacy/client), [`legacy/server`](legacy/server) | `horizon_client::mod_api` (Trait-Mods, `horizon-mixin.toml`) |
| Nightly vom 22.11.2026 (`f1bb66f`) und neuer, mit Mod-SDK | [`client`](client), [`server`](server) | `horizon-mod-api` (`horizon-mod.toml`) |

Der Launcher sucht sich selbst die passenden Ordner: Der bisherige kennt nur `horizon-mixin.toml`,
der neue nur `horizon-mod.toml`. Die Befehle unter [Installieren](#installieren) sind für beide
gleich. Beide Fassungen sprechen dasselbe Protokoll und verstehen sich untereinander.

Was in der Fassung für die bisherige API anders ist:

- **Der Host einer LAN-Welt kann nicht mitreden**, seine Gäste untereinander schon. Die bisherige
  API zeigt einem Server-Mod nur die Spieler, die sich verbinden, nicht den Spieler im Fenster des
  Hosts. Auf einem dedizierten Server gibt es die Einschränkung nicht.
- Es gibt **kein Sprachchat-Menü und keine Gruppen**: Die bisherige API erlaubt keine eigenen
  Bildschirme. Die Tasten stehen unter **Optionen > Mods > Horizon Voice Chat**, und die
  Einstellungen sind Knöpfe, die bei jedem Klick zum nächsten Wert springen. Der Server-Mod kann
  Gruppen trotzdem, für Java-Spieler mit Simple Voice Chat und Clients mit der neuen API.
- Die Texte im Spiel sind englisch, weil die bisherige API einem Mod keine Sprachdateien erlaubt.
- Die Server-Einstellungen werden nur beim Start des Servers gelesen.

## Die zwei Mods

| Ordner | Mod-ID | Seite | Aufgabe |
| --- | --- | --- | --- |
| [`client/`](client) | `horizon_voice` | `client` | Mikrofon, Wiedergabe, Tasten, HUD. Blockiert nie den Beitritt zu einem Server. |
| [`server/`](server) | `horizon_voice_server` | `server` | Vergibt die Schlüssel, betreibt den Sprachserver (UDP) und verteilt die Sprache an Spieler in Reichweite. |

Es sind zwei getrennte Mods, weil Horizon bei einem `side = "both"`-Mod jeden Client ohne diesen Mod
ablehnt, auch Java-Clients. Mit einem reinen Server-Mod bleibt der Server für alle offen.
Für die bisherige API gilt dieselbe Aufteilung mit `legacy/client/` und `legacy/server/` (gleiche
Mod-IDs); dort braucht der Server-Mod so auch keine Audio-Bibliothek.

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

Im bisherigen Launcher heißt der Ordner für den dedizierten Server `horizon-voice-chat/legacy/server`.

Die Mods sind auf ihren Horizon-Stand gepinnt: `legacy/` auf `9f42064` vom 13.11.2026
(`[compatibility]` in `horizon-mixin.toml`), `client/` und `server/` auf `f1bb66f` vom 22.11.2026
(`[native]` in `horizon-mod.toml`). Der neue Launcher baut einen Mod nur gegen genau den gepinnten
Stand; für einen anderen dort in beiden Mod-Ordnern `cargo horizon-mod update --horizon <Build>`
ausführen. Der bisherige Launcher fragt bei einem anderen Stand nach ("Build anyway").

Linux: Der Client braucht ALSA (`libasound2`), zum Bauen auch `libasound2-dev` und `pkg-config`.
Der Server-Mod hat keine Audio-Abhängigkeit.

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
                                                        # beide AES-GCM- und beide Opus-Umsetzungen gegeneinander
cd client && cargo test                                 # zusätzlich Mischer und Mikrofon-Logik
tools/check-legacy.sh pfad/zum/horizon-checkout         # baut und testet legacy/ im Horizon-Quelltext (9f42064)
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
shared/protocol/   Das Protokoll (Quelle). tools/sync-protocol.sh kopiert es in alle vier Mods,
                   weil ein Native-Mod keine Dateien außerhalb seines Ordners verwenden darf.
shared/legacy/     gcm.rs: AES-GCM auf dem Blockchiffre `aes`, für die bisherige API
client/src/        net.rs (UDP-Verbindung), audio.rs (Geräte), codec.rs (Opus), mixer.rs, lib.rs (Hooks)
client/src/        ui.rs (Menüs), icons.rs (eigene Symbole), settings.rs (Einstellungsdatei)
server/src/        udp.rs (Sprachserver), groups.rs (Gruppen), config.rs, lib.rs (Hooks)
legacy/client/     Dasselbe für die bisherige API: eigene lib.rs und codec.rs, der Rest sind Kopien
legacy/server/     Dasselbe für die bisherige API: eigene lib.rs, der Rest sind Kopien
interop-tests/     Client- und Server-Netzcode gegeneinander, ohne Spiel
PROTOCOL.md        Beschreibung des Protokolls
```

Nach Änderungen an `shared/`, an `net.rs`, `mixer.rs`, `audio.rs`, `udp.rs`, `groups.rs` oder `config.rs`
`tools/sync-protocol.sh` ausführen: Es verteilt die Dateien auf die Mods (`--check` prüft nur).

**Bisherige API (`legacy/`):** Ein Trait-Mod darf nur Crates verwenden, die in Horizons eigener
`Cargo.lock` stehen. Dort gibt es `aes`, aber nicht `aes-gcm`, und für Opus `rusty-opus` statt
libopus. Deshalb haben diese Mods eine eigene GCM-Umsetzung (`shared/legacy/gcm.rs`) und eine
eigene `codec.rs`; die Interop-Tests prüfen beide gegen die Fassungen der anderen Mods.
`tools/check-legacy.sh` baut die Mods so, wie es der Launcher tut: als Pakete in Horizons Workspace.

**Neue API (`client/`, `server/`):** Zum lokalen Kompilieren ohne Launcher erwartet jeder Mod das SDK unter `.horizon/sdk/api`
(`cargo horizon-mod` legt es an; ein Symlink auf einen Checkout von `horizon-rust/horizon` geht
auch). Ein lokales `cargo check` oder `cargo test` im Mod-Ordner schreibt `Cargo.lock` um. Die
eingecheckte `Cargo.lock` ist die Abhängigkeits-Sperre, die der Launcher verlangt: vor dem
Committen mit `git checkout -- Cargo.lock` zurücksetzen, oder nach geänderten Abhängigkeiten mit
`horizon-launcher native-lock <ordner> --update-lock` (ersatzweise `tools/make-lock.sh`) neu
erzeugen.

## Herkunft und Lizenz

Dieser Mod ist eine eigenständige Neuentwicklung in Rust unter der [MIT-Lizenz](LICENSE). Er ist
kein Port und kein Fork von Simple Voice Chat und enthält keinen Code daraus. Simple Voice Chat ist
von Max Henkel ([henkelmax](https://github.com/henkelmax)) und steht unter "All Rights Reserved".
Nachgebaut ist nur das Netzwerkformat, damit beide Mods miteinander reden können; es ist in
[PROTOCOL.md](PROTOCOL.md) beschrieben. Dieses Projekt ist nicht mit Simple Voice Chat, Horizon,
Mojang oder Microsoft verbunden und wird von ihnen nicht unterstützt.

Verwendete Bibliotheken: [cpal](https://github.com/RustAudio/cpal) (Apache-2.0),
[unsafe-libopus](https://github.com/DCNick3/unsafe-libopus) (BSD-3-Clause, libopus in Rust),
[rusty-opus](https://github.com/Remade-With-Rust/rusty-opus) (BSD-3-Clause, in `legacy/`),
[aes](https://github.com/RustCrypto/block-ciphers) (MIT/Apache-2.0),
[aes-gcm](https://github.com/RustCrypto/AEADs), [hmac](https://github.com/RustCrypto/MACs),
[sha2](https://github.com/RustCrypto/hashes) (MIT/Apache-2.0), [getrandom](https://github.com/rust-random/getrandom)
(MIT/Apache-2.0). Die Mods bauen gegen die Horizon-Mod-API (LGPL-3.0-or-later; der Stand vom
13.11.2026 steht noch unter MIT).
