# Horizon Voice Chat

Proximity-Sprachchat für [Horizon](https://github.com/horizon-rust/horizon) (Minecraft Java 26.3 in Rust):
Du hörst die Spieler in deiner Nähe, leiser mit der Entfernung und aus der Richtung, in der sie stehen.

Der Mod spricht das Netzwerkprotokoll von
[Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat) (Kompatibilitätsversion 21, also
Simple Voice Chat 2.6.x / 2.7.x für Minecraft 26.3). Dadurch können auf demselben Server
Horizon-Spieler mit diesem Mod und Java-Spieler mit dem originalen Simple Voice Chat miteinander
reden.

> **Stand: 0.1.0, ungetestet im Spiel.** Beide Mods kompilieren gegen die Horizon-Mod-API, und die
> Netzwerk-, Codec- und Mischer-Tests laufen durch. In einem echten Horizon-Client, auf einem echten
> Horizon-Server und gegen einen echten Simple-Voice-Chat-Client ist der Mod noch nicht gelaufen.
> Siehe [Testen](#testen).
>
> **Welcher Horizon-Stand:** Die Mods sind gegen die Mod-API `0.2` aus Horizons `main`-Zweig vom
> 16.11.2026 (Commit `c07bfa76`) geschrieben. Horizon hat sein Mod-System kurz davor umgebaut: Das
> Nightly vom 13.11.2026 (`9f42064`) hat noch die ältere API (`horizon_client::mod_api`, Bau aus dem
> Spiel-Quelltext) und kann diese Mods nicht bauen. Für den neuen Weg war am 16.11.2026 außerdem
> noch kein SDK veröffentlicht (kein `sdk-…`-Release). Bis ein Horizon-Build mit SDK erscheint,
> lässt sich der Mod nur gegen einen eigenen Horizon-Checkout bauen (siehe [Entwickeln](#entwickeln)).

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

Die Mods sind auf den Horizon-Stand `c07bfa76` vom 16.11.2026 gepinnt (`[native]` in
`horizon-mod.toml`). Der Launcher baut einen Mod nur gegen genau den gepinnten Stand. Für einen
anderen Stand in beiden Mod-Ordnern `cargo horizon-mod update --horizon <Build>` ausführen.

Linux: Der Client braucht ALSA (`libasound2`), zum Bauen auch `libasound2-dev` und `pkg-config`.
Der Server-Mod hat keine Audio-Abhängigkeit.

## Bedienen

| Taste (Standard) | Wirkung |
| --- | --- |
| `V` | Push-to-Talk: sprechen, solange gehalten |
| nicht belegt | Flüstern: beim Sprechen halten, trägt nur halb so weit |
| `M` | Mikrofon stumm |
| `N` | Sprachchat ganz aus (du hörst und sendest nichts) |

Die Tasten stehen unter **Optionen > Steuerung > Sprachchat**. Unter **Optionen > Mods > Horizon
Voice Chat** gibt es Push-to-Talk oder Sprachaktivierung, den Schwellwert, Mikrofon- und
Ausgabelautstärke und die HUD-Anzeige. Unten links zeigt das HUD, ob du sprichst, und wer gerade
spricht.

## Server einstellen

Der Server legt `config/horizon_voice_server-server.toml` an (pro Welt unter `serverconfig/`
überschreibbar):

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

Port, Reichweiten und Keep-Alive gelten nach einem Neustart des Servers.

## Was (noch) fehlt

- **Gruppen** (private Gesprächsräume), Lautstärke-Kategorien, Aufnahme und die Plugin-API von
  Simple Voice Chat. Der Server meldet "Gruppen aus", Original-Clients blenden die Gruppen dann aus.
- **Lautstärke pro Spieler**, Auswahl von Mikrofon und Lautsprecher (es gelten die Standardgeräte
  des Systems), Rauschunterdrückung.
- **Zuschauer-Sonderregeln** und vom Server erzwungenes Stummschalten.
- **SRV-Einträge:** Der Client schickt Sprache an den Hostnamen, mit dem du beigetreten bist. Zeigt
  ein SRV-Eintrag auf einen anderen Rechner, muss der Server `voice_host` setzen.
- **Ältere Protokollversionen:** Nur Version 21. Ändert Simple Voice Chat die Version, muss dieser
  Mod nachziehen.

## Testen

Was automatisch geprüft ist:

```sh
cargo test --manifest-path interop-tests/Cargo.toml   # Protokoll, Client- und Server-Netzcode über Loopback
cd client && cargo test                                 # zusätzlich Opus, Mischer, Mikrofon-Logik
```

Was im Spiel noch geprüft werden muss:

1. Baut der Launcher beide Mods (`native-build`), und lädt Horizon sie ohne Ablehnung?
2. Zwei Horizon-Clients auf einem Horizon-Server: Verbinden, hören, Richtung, Entfernung, Dimension.
3. Ein Java-Client mit Simple Voice Chat auf demselben Server: Hören beide Seiten einander?
4. Ein Horizon-Client auf einem Paper-Server mit Simple Voice Chat.
5. LAN-Host mit beiden Mods.

Im Log stehen die Zeilen des Mods unter `Voice chat …`.

## Entwickeln

```
shared/protocol/   Das Protokoll (Quelle). tools/sync-protocol.sh kopiert es in beide Mods,
                   weil ein Native-Mod keine Dateien außerhalb seines Ordners verwenden darf.
client/src/        net.rs (UDP-Verbindung), audio.rs (Geräte), codec.rs (Opus), mixer.rs, lib.rs (Hooks)
server/src/        udp.rs (Sprachserver), config.rs, lib.rs (Hooks)
interop-tests/     Client- und Server-Netzcode gegeneinander, ohne Spiel
PROTOCOL.md        Beschreibung des Protokolls
```

Zum lokalen Kompilieren ohne Launcher erwartet jeder Mod das SDK unter `.horizon/sdk/api`
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
[aes-gcm](https://github.com/RustCrypto/AEADs), [hmac](https://github.com/RustCrypto/MACs),
[sha2](https://github.com/RustCrypto/hashes) (MIT/Apache-2.0), [getrandom](https://github.com/rust-random/getrandom)
(MIT/Apache-2.0). Die Mods bauen gegen die Horizon-Mod-API (LGPL-3.0-or-later).
