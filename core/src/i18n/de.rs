//! The core's German: every text it shows, by its English (the code holds the English).
//! Grouped by the file the texts are from.

use super::Table;

/// German, by the English the code holds.
#[rustfmt::skip]
pub const DE: Table = &[
    // Words used in several places:
    ("now", "jetzt"),
    ("Weekly", "Woche"),
    ("5-hour", "5 Stunden"),
    ("On", "An"),
    ("Off", "Aus"),
    // From alerts.rs:
    ("Your week: {0} prompts, {1} tokens", "Deine Woche: {0} Prompts, {1} Token"),
    ("Your week: 1 prompt, {0} tokens", "Deine Woche: 1 Prompt, {0} Token"),
    ("Weekly limit at {0} %", "Wochenlimit bei {0} %"),
    ("Resets {0}", "Reset {0}"),
    ("The weekly limit could run out early", "Das Wochenlimit könnte früher aufgebraucht sein"),
    ("5-hour limit at {0} %", "5-Stunden-Limit bei {0} %"),
    ("The weekly limit is free again", "Das Wochenlimit ist wieder frei"),
    ("The 5-hour limit is free again", "Das 5-Stunden-Limit ist wieder frei"),
    ("Weekly and 5-hour limits are free again", "Wochen- und 5-Stunden-Limit sind wieder frei"),
    // From extensions.rs:
    ("in {0}", "in {0}"),
    ("{0} ago", "vor {0}"),
    // From mqtt.rs:
    ("No mqtt:// address on the clipboard", "Keine mqtt://-Adresse in der Zwischenablage"),
    ("The address isn't a host name or IP address", "Die Adresse ist kein Hostname und keine IP-Adresse"),
    ("The port is a number from 1 to 65535", "Der Port ist eine Zahl von 1 bis 65535"),
    ("the broker doesn't speak MQTT 3.1.1", "der Broker spricht kein MQTT 3.1.1"),
    ("the broker refused the client id", "der Broker lehnt die Client-ID ab"),
    ("the broker is unavailable", "der Broker ist nicht verfügbar"),
    ("wrong user name or password", "Benutzer oder Passwort falsch"),
    ("not authorised: the broker refused this user name and password", "nicht berechtigt: der Broker lehnt diesen Benutzernamen und dieses Passwort ab"),
    ("the broker answered {0}", "der Broker antwortete {0}"),
    ("{0} not found ({1})", "{0} nicht gefunden ({1})"),
    ("{0} not found", "{0} nicht gefunden"),
    ("TLS: {0}", "TLS: {0}"),
    ("no answer from the broker", "keine Antwort vom Broker"),
    ("connection lost", "Verbindung verloren"),
    // From push.rs:
    ("The demo sends nothing", "Die Demo sendet nichts"),
    ("The server answered {0}", "Der Server antwortete {0}"),
    ("The server answered {0}: check the token", "Der Server antwortete {0}: prüfe das Token"),
    ("That isn't an http:// or https:// address", "Das ist keine http://- oder https://-Adresse"),
    ("Enter the address", "Gib die Adresse ein"),
    ("Enter the topic", "Gib das Thema ein"),
    ("A topic has letters, digits, - and _ only", "Ein Thema hat nur Buchstaben, Ziffern, - und _"),
    ("The token has a space or another character it can't have", "Das Token hat ein Leerzeichen oder ein anderes unzulässiges Zeichen"),
    ("Enter the server's address", "Gib die Adresse des Servers ein"),
    ("The server isn't an http:// or https:// address", "Der Server ist keine http://- oder https://-Adresse"),
    ("Enter the app's token (Gotify → Apps)", "Gib das Token der App ein (Gotify → Apps)"),
    // From tracking.rs:
    ("Can't read {0}: {1}", "{0} lässt sich nicht lesen: {1}"),
    ("Can't write {0}: {1}", "{0} lässt sich nicht schreiben: {1}"),
    ("Can't find this app's exe: {0}", "Die Programmdatei dieser App wurde nicht gefunden: {0}"),
    // From update.rs:
    ("GitHub answered {0}", "GitHub antwortete {0}"),
    // From usage.rs:
    ("No Claude Code login found. Run `claude` and sign in.", "Keine Claude-Code-Anmeldung gefunden. Starte `claude` und melde dich an."),
    ("Login expired. Using Claude Code renews it.", "Anmeldung abgelaufen. Claude Code zu benutzen erneuert sie."),
    ("Rate limited, retrying later.", "Zu viele Anfragen, später neuer Versuch."),
    ("Claude Code isn't used on this PC", "Claude Code wird auf diesem PC nicht benutzt"),
    ("Usage request failed (HTTP {0}).", "Abfrage der Nutzung fehlgeschlagen (HTTP {0})."),
    ("Unexpected response from the usage API.", "Unerwartete Antwort der Nutzungs-API."),
];
