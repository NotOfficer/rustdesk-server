# Bericht: moderne Verbindungsarten für den Open-Source-RustDesk-Server

Stand 24.10.2026. Branch `feature/modern-transports` in deinem Fork
`NotOfficer/rustdesk-server`. Referenz ist der Client 1.5.0.

## Kurzfassung

Der Open-Source-Server kann jetzt alles, was der Client 1.5.0 an
Verbindungsarten anbietet. Ich habe das im Labor mit den offiziellen
Client-Paketen nachgewiesen, nicht nur in Tests.

| Verbindungsart | Ergebnis |
|---|---|
| Schlüsselaustausch, also eine verschlüsselte Verbindung zwischen Client und Server. Version 1 wie in 1.5.0, Version 0 für ältere Clients | funktioniert |
| IPv6-Direktverbindung | funktioniert |
| WebRTC | funktioniert, auch im selben LAN |
| UDP-Hole-Punching | funktioniert, auch hinter NAT-Routern |

Ältere Clients verbinden sich genau wie vorher. Im Labor liefen echte Clients
1.4.9 und 1.4.0, im Code habe ich zusätzlich 1.4.1 geprüft. Clients mit
ausgeschalteten Optionen verhalten sich wie beim Original. Jede Funktion lässt
sich im Server einzeln abschalten. Datenbank und Schlüsseldateien bleiben
unverändert, der Weg zurück zum offiziellen Image ist also jederzeit offen.

Was nicht geht oder nicht zur Aufgabe gehörte:

- Clients, die sich über TCP oder WebSocket am Server anmelden, unterstützt
  der Server weiterhin nicht. Das betrifft Clients hinter einem Proxy, den
  Web-Client und die Option `disable-udp`. Beim Original ist das genauso.
- WebRTC über die WebSocket-Ports 21118/21119 habe ich absichtlich nicht
  aktiviert. Die Aufgabe verlangt, dass sich WebSocket-Clients exakt wie
  vorher verhalten.
- Der Relay-Server hbbr hat einen alten Fehler, den ich nicht angefasst habe.
  Treffen beide Relay-Anfragen einer Sitzung genau gleichzeitig ein, muss der
  Client es erneut versuchen. Das Original hat denselben Fehler. Im letzten
  Laborlauf traf er zwei Fälle mit dem Original-hbbs. Dort dauerte die
  Verbindung 60 und 86 Sekunden statt etwa 10, kam aber zustande.

## Ergebnis im Detail

### Wie ich geprüft habe

Alle drei Ebenen laufen mit einem Befehl, `lab/run-all-tests.sh`.

1. **Bauen und statische Prüfung.** Der Server baut. Das Prüfwerkzeug
   `clippy` meldet keine neue Warnung im Server-Code, es sind 72 Meldungen
   statt vorher 90.
2. **Protokolltests.** 124 automatische Tests starten ein echtes hbbs und
   spielen die Nachrichtenfolgen nach, die die Clients 1.5.0 und 1.4.x
   senden. Ein Vergleichstest schickt dieselben alten Anfragen an das
   Original-hbbs und an das neue und verlangt Feld für Feld dieselben
   Antworten.
3. **Labor mit echten Clients.** In Docker laufen das Original-hbbs oder das
   neue, dazu hbbr, ein STUN-Server und zwei offizielle RustDesk-Clients
   ohne Bildschirm unter Xvfb. Der steuernde Client baut eine Verbindung auf.
   Aus seinem Log liest das Skript zwei Zeilen ab. `#1 ... punch attempt`
   zeigt, was er angeboten hat, `used to establish ... connection`, was
   gewonnen hat.

### Belege aus dem Labor

Aus dem letzten Komplettlauf, Commit `61a6a7f`. Die Zeilen stammen aus dem
Log des steuernden Clients.

| Fall | angeboten | verwendet |
|---|---|---|
| Original-hbbs, Standard | `#1 TCP punch attempt` | `used to establish TCP connection with TCP punch` |
| neues hbbs, neue Optionen aus | `#1 TCP punch attempt` | `used to establish TCP connection with TCP punch` |
| nur UDP | `#1 UDP punch attempt` | `used to establish UDP connection with UDP punch` |
| nur IPv6 | `#1 IPv6 punch attempt` | `used to establish IPv6 connection with IPv6 punch` |
| nur WebRTC | `#1 WebRTC punch attempt` | `used to establish WebRTC connection with WebRTC punch` |
| WebRTC im selben LAN | `#1 WebRTC punch attempt` | `used to establish WebRTC connection with WebRTC punch` |
| UDP hinter NAT-Routern | `#1 UDP punch attempt` | `used to establish UDP connection with UDP punch` |
| alles an, hinter NAT-Routern | `#1 UDP+IPv6+WebRTC punch attempt`, parallel `#1 TCP punch attempt` | `used to establish UDP connection with UDP+IPv6+WebRTC punch` |

Dazu Zeilen aus dem hbbs-Log:

- Schlüsselaustausch Version 1 mit dem Client 1.5.0:
  `Tcp connection from [::ffff:198.18.10.21]:46807 secured, version 1`
- UDP-Antwort an den Controller, nur über dessen TCP-Verbindung:
  `UDP punch hole response to 198.18.10.22:39895 from [::ffff:198.18.10.21]:48791, is_udp: true`
- WebRTC im selben LAN. hbbs loggt nur die Größe des Angebots, nie den Inhalt:
  `WebRTC offer (673 bytes) from [::ffff:198.18.10.31]:34089 to "36555271"`

Im Fall "alles an" ohne NAT gewinnt meist TCP. Ohne Router klappt im Labor
jede Direktverbindung sofort, und der Client nimmt die schnellste. Das ist
gewolltes Client-Verhalten.

## Testmatrix

Letzter Komplettlauf: OVERALL PASS in 14 min 35 s, Ergebnis in
`lab/out/all-20261024-095230/all-tests-summary.txt`.

| Ebene | Prüfung | Ergebnis |
|---|---|---|
| 1 | `cargo build --release` | PASS |
| 1 | `cargo clippy`, keine neue Warnung im Server-Code | PASS |
| 1 | clippy im mitgelieferten hbb_common | WARN, 2 neue Meldungen im fremden Upstream-Code, der mit dem vorgeschriebenen Update auf den Stand von 1.5.0 kam |
| 2 | Unit-Tests 39 + 7, IPv6 15, Schlüsselaustausch 18, UDP 19, WebRTC 22, Vergleich mit dem Original 3, sonstige 1 | alle PASS |
| 3 | 28 Labor-Fälle | alle PASS |

Die Labor-Fälle. Wo nicht anders angegeben, laufen Clients 1.5.0.

| Fall | Bedeutung | Ergebnis |
|---|---|---|
| baseline | Original-hbbs, Standardoptionen | TCP ✔ |
| baseline-allopts | Original-hbbs, alle Optionen an | TCP ✔, das Original kann nur TCP und Relay |
| modern-off | neues hbbs, neue Optionen im Client aus | TCP ✔, wie das Original |
| modern-switches-off | neues hbbs mit allen vier Schaltern aus, Client alles an | TCP ✔, wie das Original |
| udp-only / ipv6-only / webrtc-only | je eine Verbindungsart, TCP-Punch aus | UDP ✔ / IPv6 ✔ / WebRTC ✔ |
| all-on | alles an | direkt ✔ |
| old-client | 1.4.9 auf beiden Seiten | TCP ✔ |
| baseline-old-allopts / old-client-allopts | 1.4.9 mit allem an, gegen Original / neu | beide TCP ✔ |
| mixed-old-controlled / mixed-old-controller | 1.5.0 und 1.4.9 gemischt | TCP ✔ |
| client-1.4.0 / baseline-client-1.4.0 | 1.4.0 auf beiden Seiten, neu / Original | TCP ✔ |
| mixed-1.4.0-controller / mixed-1.4.0-controlled | 1.4.0 und 1.5.0 gemischt | TCP ✔ |
| webrtc-old-controlled | 1.5.0 will nur WebRTC, die Gegenseite 1.4.9 kann es nicht | Relay ✔, wie das Original |
| baseline-webrtc-old-controlled | dasselbe mit dem Original-hbbs | Relay ✔ |
| lan-baseline / lan-default | beide im selben LAN, Original / neu | TCP über das LAN ✔ |
| lan-webrtc / lan-all-on | selbes LAN, WebRTC / alles | WebRTC ✔ / direkt ✔ |
| nat-baseline | hinter NAT-Routern, Original | Relay ✔ |
| nat-udp-only / nat-ipv6-only / nat-webrtc-only / nat-all-on | hinter NAT-Routern, neu | UDP ✔ / IPv6 ✔ / WebRTC ✔ / UDP ✔ |

Zu den NAT-Fällen: Die Linux-Router im Labor verwerfen TCP-Punching wegen
ihrer strengen Verbindungsverfolgung. Dort landet "nur TCP" deshalb beim
Relay. Echte Heimrouter verhalten sich unterschiedlich, und genau für sie
sind UDP, IPv6 und WebRTC da.

### Testskript ausführen

Du brauchst einen Ubuntu-Rechner mit Docker, sudo für Docker genügt. Im
Repo-Verzeichnis:

```sh
lab/run-all-tests.sh                                # alles, Client 1.5.0
lab/run-all-tests.sh -v 1.5.1                       # mit einer neueren Client-Version
lab/run-all-tests.sh --skip 3                       # nur Ebene 1 und 2, ohne Labor
lab/run-all-tests.sh --cases udp-only,webrtc-only   # nur bestimmte Labor-Fälle
```

Ein Lauf dauert 13 bis 15 Minuten, der allererste etwa 20, weil er die Images
baut. Am Ende steht eine PASS/FAIL-Tabelle, die das Skript unter
`lab/out/<lauf>/all-tests-summary.txt` ablegt. Bei einem Fehler endet es mit
einem Fehlercode. Es baut alles selbst, braucht keine Bedienung und räumt das
Labor danach ab. Mehr dazu in `lab/README.md`.

## Entscheidungen und Risiken

Alle Entscheidungen mit Begründung stehen in `DECISIONS.md`. Die technische
Beschreibung mit Nachrichtenabläufen, Sicherheitsdesign und Kompatibilität
steht in `docs/modern-transports.md`. Die wichtigsten Entscheidungen:

- **Neue Direktverbindungen gibt es nur für Clients ab 1.5.0.** Clients 1.4.x
  senden zum Teil schon UDP- und IPv6-Angaben. Scheitert der Versuch, warten
  sie aber bis zu 20 Sekunden, bevor sie auf das Relay ausweichen. Damit sich
  für sie nichts verschlechtert, behandelt der Server sie wie das Original.
  Das gilt für 1.4.x als steuernde Seite und, sobald der Server die Version
  kennt, auch als gesteuerte Seite.
- **Ein erzwungenes Relay bleibt erzwungen.** Mit `ALWAYS_USE_RELAY=Y` oder
  "immer über Relay" im Client vermittelt der Server kein UDP, IPv6 oder
  WebRTC, weil diese das Relay sonst umgehen würden.
- **WebSocket bleibt unverändert**, wie gefordert.
- **WebRTC braucht den Server-Key auf dem steuernden Client und meist auch
  auf dem gesteuerten.** Ohne Key kann die gesteuerte Seite ihre
  WebRTC-Antwort nur über den UDP-Weg schicken. Ist UDP-Hole-Punching aus,
  fällt die Verbindung nach dem zweiten Versuch auf den normalen Weg zurück,
  etwa 9 Sekunden später. Über den UDP-Weg kommt die Antwort unverschlüsselt
  beim Server an. So hat RustDesk den Client gebaut, die Sitzung selbst
  bleibt durch die eigene Schlüsselprüfung geschützt.
- **Antworten zählen nur vom Rechner der Gegenseite.** Eine WebRTC-Antwort,
  eine IPv6-Adresse oder eine UDP-Antwort leitet der Server nur weiter, wenn
  sie von der Adresse kommt, mit der sich die Gegenseite angemeldet hat.
  Sonst könnte jeder, der die Adresse eines wartenden Controllers kennt,
  dessen Verbindungsversuch kapern.

### Offene Risiken

1. Ich konnte nicht über das echte Internet testen. Das Labor bildet NAT-Router
   nach, aber echte Heimrouter, Mobilfunk-NAT und Firmen-Firewalls verhalten
   sich unterschiedlich. Kommen UDP, IPv6 oder WebRTC nicht durch, fällt der
   Client wie bisher auf TCP oder Relay zurück.
2. Im Labor liefen nur Linux-Clients, die offiziellen .deb-Pakete. Windows,
   macOS und die Mobil-Clients sprechen dasselbe Protokoll, ich habe sie aber
   nicht ausgeführt.
3. Angemeldete Clients, also solche mit Konto auf einem API-Server, nutzen den
   Schlüsselaustausch. Das decken die Protokolltests ab. Im Labor ging es
   nicht, weil ein API-Server fehlt. Mit dem Original-Server scheiterten
   solche Clients nach 18 Sekunden. Jetzt funktioniert es, wenn der Key
   stimmt.
4. Ein Angreifer mit sehr vielen IP-Adressen und dem öffentlichen Server-Key
   kann die neuen Tabellen füllen. Dann fallen UDP, IPv6 und WebRTC für neue
   Verbindungen weg, es geht aber wie beim Original über TCP oder Relay
   weiter. Während einer UDP-Flut verlieren Clients mit hoher Latenz das
   UDP-Hole-Punching, mit demselben Rückfall. Mit dem Server-Key kann ein
   Angreifer außerdem einem Rechner, der den Server über IPv4 erreicht, die
   IPv6-Direktverbindung abdrehen. Dazu nennt er dessen IPv6-Adressbereich
   oft genug als Ziel. Auch dann geht es über die anderen Wege weiter.
5. Bei UDP, IPv6 und WebRTC schicken die Clients selbst kleine Prüfpakete an
   Adressen, die der Server vermittelt. So funktioniert Hole-Punching
   überall, auch bei Server Pro. Der Server begrenzt das: nur globale
   IPv6-Adressen, Mengenlimits pro Absender und pro Ziel, und jede Anfrage
   braucht den Lizenzschlüssel.
6. Das Wettlaufproblem in hbbr aus der Kurzfassung habe ich nicht angefasst.
   Eine zweite alte Schwäche ist jetzt größtenteils behoben. Ein
   fehlgeschlagener UDP-Versand blockierte alle weiteren UDP-Sendungen, bis
   der Server den Socket neu aufbaute. Ein einziges zu großes Relay-Paket
   reichte dafür. Weitergeleitete Nachrichten und die neue UDP-Antwort gehen
   jetzt direkt hinaus. Nur die Antworten auf Anmeldungen nutzen noch den
   alten Weg aus dem Original-Code.
7. Die gesteuerte Seite meldet ihre Version erst mit ihrer ersten Antwort. Bis
   dahin kann ein 1.4.x-Client einmal einen Umweg über UDP nehmen, bevor die
   parallele TCP-Runde des Controllers gewinnt. Kommen seine Antworten von
   einer anderen IP als seine Anmeldung, merkt der Server sich die Version
   nie. Dann kann der Umweg bei jeder Verbindung vorkommen.
8. Manche Router mit mehreren Internetanschlüssen schicken jede Verbindung
   über einen anderen Anschluss. Ein gesteuerter Rechner dahinter verliert
   das UDP-Hole-Punching, weil seine UDP-Antwort von einer anderen Adresse
   kommt als seine Anmeldung. Die parallele TCP-Runde des Controllers
   übernimmt dann. Ist TCP-Punching im Controller abgeschaltet, landet die
   Verbindung nach einigen Sekunden beim Relay.
9. Ein Vergleichstest schlug früher einmal sporadisch fehl. Die
   wahrscheinliche Ursache war ein Port-Wettlauf im Test-Harness. Zwei
   gleichzeitig startende Test-Server konnten sich einen Port teilen. Das
   habe ich behoben, seitdem kam der Fehler nicht wieder vor.
10. Künftige Client-Versionen können das WebRTC-Format oder den
    Schlüsselaustausch ändern. Was dann zu prüfen ist, steht am Ende.

## Reviews

Nach jedem Feature hat ein frischer, unabhängiger Reviewer den Diff
geprüft, Opus 5.5 auf höchster Stufe. Er bekam nur die Aufgabenbeschreibung,
den Diff und den Client-Code, keine Erklärungen von mir. Danach folgte der
Review-Loop über den gesamten Diff mit höchstens 8 Runden.

| Review | Befunde | gültig | schwer | behoben |
|---|---|---|---|---|
| Feature-Reviews: Schlüsselaustausch, IPv6, WebRTC, UDP | 17 | 17 | 2 | 17 |
| Gesamt-Review, Runden 1 bis 8 | 20, je Runde 4, 4, 2, 3, 3, 1, 1, 2 | 20 | 0 | 20 |
| Zusatz-Review GPT 6.1 Sol, auf deinen Wunsch | 2 | 2 | 1 | 2 |
| Zusatz-Review Opus 5.5, auf deinen Wunsch | 4 | 4 | 0 | 4, davon 2 nur in der Doku |

Keine Runde blieb ganz ohne Befund, aber die Funde wurden immer kleiner. Der
Loop endete wie vorgegeben nach Runde 8. Die Fixes aus Runde 8 sind getestet,
ein weiterer Reviewer hat sie nicht mehr gesehen.

Danach hast du zwei weitere Gesamt-Reviews angefordert, mit derselben
Aufgabenbeschreibung und denselben Regeln. Eines kam von GPT 6.1 Sol, eines
von einem frischen Opus 5.5, beide auf höchster Stufe. Das hat sich gelohnt.
GPT fand zwei echte Lücken, die alle acht Runden übersehen hatten. Bei der
schwereren konnte ein manipulierter gesteuerter Client dem Controller eine
IPv6-Adresse unterschieben, selbst wenn der Server das Relay erzwang. Bei der
zweiten konnte jeder, der die Adresse eines wartenden Controllers kannte, ihm
eine falsche WebRTC-Antwort schicken oder seinen UDP-Versuch verbrauchen.
Beides ist behoben, mit Tests, die auf dem alten Stand fehlschlagen.

Opus fand vier kleinere Punkte. Ein zu großes Relay-Paket konnte den
UDP-Socket blockieren, ein alter Fehler aus dem Original. Das ist behoben.
Ein Fremder mit dem Server-Key konnte das IPv6-Budget eines anderen
verbrauchen. Das ist für Rechner behoben, die den Server über IPv6
erreichen. Zwei Stellen in der Doku waren falsch. Einen Vorschlag habe ich
bewusst nicht übernommen, weil er eine in Runde 8 geschlossene Lücke wieder
geöffnet hätte. Auch diese letzten Fixes hat kein weiterer Reviewer mehr
gesehen. Sie haben Tests und bestehen alle drei Testebenen.

Die beiden schweren Befunde der Feature-Reviews kamen aus dem IPv6-Review.
Ältere Clients mit erzwungenem Relay hätten über IPv6 trotzdem direkt
verbinden können, das Relay wäre also umgangen worden. Und der Server gab
die vom Controller genannte IPv6-Adresse ungeprüft an die Gegenseite weiter,
die dorthin Prüfpakete schickt.

Die kleineren Funde betrafen vor allem das Zeitverhalten des
Schlüsselaustauschs, die Fairness der Tabellen unter Last, Randfälle der
Rückwärtskompatibilität und Testlücken. Ein Beispiel ist 1.4.1, das als
einzige 1.4er-Version keine parallele TCP-Runde hat. Jeder Befund steht mit
Begründung und Fix-Commit in `REVIEWS.md`.

## Image bauen und starten

Auf einem Rechner mit Docker, im Repo-Verzeichnis:

```sh
docker build -f docker-classic/Dockerfile.multistage -t rustdesk-server-modern:dev .
```

Das Image ist aufgebaut wie das offizielle `rustdesk/rustdesk-server`. Es
enthält nur `hbbs` und `hbbr`, das Arbeitsverzeichnis ist `/root`. Du kannst
es deshalb genauso starten wie bisher und in deiner `docker-compose.yml` nur
`image:` austauschen:

```yaml
services:
  hbbs:
    image: rustdesk-server-modern:dev
    command: hbbs
    volumes:
      - ./data:/root          # gleiche Schlüssel und Datenbank wie bisher
    ports:
      - 21115:21115
      - 21116:21116
      - 21116:21116/udp       # wichtig für UDP-Hole-Punching
      - 21118:21118
  hbbr:
    image: rustdesk-server-modern:dev
    command: hbbr
    volumes:
      - ./data:/root
    ports:
      - 21117:21117
      - 21119:21119
```

Zurück zum offiziellen Image kommst du, indem du `image:` wieder
zurückstellst. Datenbank und Schlüssel passen weiter.

### Fertiges Image von GitHub

Selbst bauen musst du nicht. Der Workflow "publish image" baut das Image auf
GitHub und legt es in der GitHub Container Registry ab. Im Fork klickst du
auf Actions, links auf "publish image" und rechts auf "Run workflow". Unter
"Use workflow from" wählst du den Branch, ein zusätzlicher Tag wie `latest`
ist optional. Nach einigen Minuten liegt das Image unter
`ghcr.io/notofficer/rustdesk-server:<branch>`, Schrägstriche im Branch-Namen
werden zu Bindestrichen. Für unseren Branch heißt das:

```yaml
    image: ghcr.io/notofficer/rustdesk-server:feature-modern-transports
```

Meldet `docker pull` auf deinem Server "denied", ist das Paket noch privat.
Dann stellst du es auf GitHub unter deinem Profil, Packages,
rustdesk-server, Package settings auf Public. Alternativ meldest du den
Server mit `docker login ghcr.io` und einem Token mit dem Recht
`read:packages` an.

### Schalter

Alle neuen Funktionen sind standardmäßig an. Abschalten kannst du sie mit
einer Umgebungsvariable unter `environment:` im Compose-File oder mit einer
Kommandozeilen-Option. `N`, `no`, `0`, `false` und `off` schalten ab.

| Funktion | Umgebungsvariable | Option |
|---|---|---|
| Schlüsselaustausch. Nötig für WebRTC und für angemeldete Clients | `SECURE_TCP` | `--secure-tcp` |
| IPv6-Direktverbindung | `IPV6_PUNCH` | `--ipv6-punch` |
| WebRTC | `WEBRTC` | `--webrtc` |
| UDP-Hole-Punching | `UDP_PUNCH` | `--udp-punch` |

Ein Beispiel: `environment: [ "WEBRTC=N" ]` schaltet nur WebRTC ab. Dazu gibt
es `UDP_TEST_NAT_LIMIT`, die Zahl der UDP-NAT-Test-Antworten pro Sekunde
insgesamt, Standard 2000. Details stehen in `docs/environment-variables.md`.

### Was auf den Clients nötig ist

- Bei einem eigenen Server sind UDP, IPv6 und WebRTC im Client standardmäßig
  aus. Du musst sie auf dem steuernden PC einschalten. In der Desktop-App
  findest du sie unter Einstellungen, Allgemein, "Weitere Einstellungen". Die
  Häkchen heißen "UDP-Hole-Punching aktivieren", "IPv6-P2P-Verbindung
  aktivieren" und "WebRTC-P2P-Verbindung aktivieren". Auf dem gesteuerten PC
  musst du nichts einschalten.
- Für WebRTC braucht der steuernde Client den richtigen Server-Key, das ist
  die Einstellung `key`. Der gesteuerte sollte ihn auch haben. Ohne Key
  klappt WebRTC dort nur zusammen mit UDP-Hole-Punching.
- Für UDP muss der UDP-Port 21116 am Server erreichbar sein.

## Bei künftigen Client-Updates prüfen

1. Die neue Version mit dem Testskript prüfen: `lab/run-all-tests.sh -v <version>`.
   Es lädt das offizielle Paket von GitHub und testet alle Verbindungsarten.
2. Im Client-Quellcode in `libs/hbb_common/protos/rendezvous.proto` nachsehen,
   ob neue Felder oder Nachrichten dazugekommen sind. Dann auch hbb_common im
   Server auf den Stand der neuen Version anheben.
3. Prüfen, ob sich der Schlüsselaustausch geändert hat, also ob
   `KX_VERSION_LATEST` in `libs/hbb_common/src/tcp.rs` größer als 1 ist. Dann
   muss hbbs die neue Version lernen. Bis dahin verhandeln neue Clients
   einfach Version 1.
4. Prüfen, ob sich das Format des WebRTC-Angebots geändert hat, heute
   `webrtc://` plus base64-JSON mit SDP. hbbs liest daraus den
   DTLS-Fingerabdruck. Kann es ihn nicht lesen, leitet es das Angebot nicht
   weiter. WebRTC fällt dann weg, die anderen Wege funktionieren weiter.
5. Prüfen, ob die Grenzen noch passen. Angebot und Antwort dürfen höchstens
   4096 Bytes groß sein, ein ICE-Kandidat 2048 Bytes, pro Richtung und Route
   gehen 128 Kandidaten-Nachrichten durch.
6. Prüfen, ob die Versionsgrenze 1.5.0 für UDP und IPv6 noch stimmt.
