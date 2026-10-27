use std::time::Duration;

pub const USER_AGENT: &str = concat!("DaPiCu/ModSync/", env!("CARGO_PKG_VERSION"));

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
        .expect("HTTP-Client konnte nicht erstellt werden")
}

/// Download-Client ohne Gesamt-Timeout (große Dateien), aber mit Verbindungs-Timeout.
pub fn download_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("HTTP-Client konnte nicht erstellt werden")
}

/// Übersetzt Netzwerkfehler in verständliche Meldungen.
pub fn describe(service: &str, err: &reqwest::Error) -> String {
    if err.is_timeout() {
        format!("{service} antwortet nicht (Zeitüberschreitung).")
    } else if err.is_connect() {
        format!("Keine Verbindung zu {service}. Internetverbindung oder Firewall prüfen.")
    } else if err.is_decode() {
        format!("{service} hat eine unerwartete Antwort geschickt ({err}).")
    } else {
        format!("Fehler bei der Anfrage an {service}: {err}")
    }
}

pub fn describe_status(service: &str, status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 | 403 if service == "CurseForge" => {
            "CurseForge lehnt den API-Key ab. Bitte in den Einstellungen prüfen.".into()
        }
        429 => format!("{service}: Zu viele Anfragen. Bitte kurz warten und erneut versuchen."),
        500..=599 => format!("{service} hat gerade Serverprobleme (HTTP {status})."),
        _ => format!("{service} antwortet mit HTTP {status}."),
    }
}
