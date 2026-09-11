//! Begrenzter lesender HTTP-Endpunkt mit ausdruecklicher Quellenfrische.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::messung::Messung;

pub struct Stand {
    pub messungen: Vec<Messung>,
    pub fehler: Option<String>,
    pub letzte_lesung: Option<Instant>,
    pub sitzung: String,
    pub revision: u64,
}

pub fn bedienen(
    horcher: TcpListener,
    stand: Arc<Mutex<Stand>>,
    wort: String,
) -> Result<(), String> {
    let aktiv = Arc::new(AtomicUsize::new(0));
    let wort = Arc::new(wort);
    for verbindung in horcher.incoming() {
        let Ok(strom) = verbindung else { continue };
        if aktiv.fetch_add(1, Ordering::AcqRel) >= 8 {
            aktiv.fetch_sub(1, Ordering::AcqRel);
            drop(strom);
            continue;
        }
        let aktiv = Arc::clone(&aktiv);
        let stand = Arc::clone(&stand);
        let wort = Arc::clone(&wort);
        // Jede Frage in ihrem eigenen Faden: eine hängende Verbindung darf
        // die nächste Messung nicht aufhalten.
        std::thread::spawn(move || {
            let _ = antworten(strom, &stand, &wort);
            aktiv.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Ok(())
}

/// Zeitkonstanter Vergleich zweier Wörter.
///
/// Ein `==` auf Zeichenketten bricht beim ersten Unterschied ab. Wer den Boten
/// im Netz erreicht, könnte daran Zeichen für Zeichen raten. Die Länge fällt
/// dabei auf, das ist hingenommen: sie ist bekannt und fest.
fn wort_stimmt(vorgezeigt: &str, erwartet: &str) -> bool {
    let a = vorgezeigt.as_bytes();
    let b = erwartet.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut unterschied: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        unterschied |= x ^ y;
    }
    unterschied == 0
}

/// Das vorgezeigte Wort aus den Kopfzeilen, falls eines dabei ist.
///
/// Liest die Kopfzeilen bis zur Leerzeile. Der Bote antwortet auf drei
/// Adressen und braucht dafür keine weitere Kopfzeile.
fn wort_aus_kopfzeilen(leser: &mut impl BufRead) -> Option<String> {
    let mut gefunden = None;
    let mut gelesen = 0;
    loop {
        let mut zeile = String::new();
        let n = (&mut *leser).take(8193).read_line(&mut zeile).ok()?;
        gelesen += n;
        if gelesen > 8192 {
            return None;
        }
        if n == 0 {
            break;
        }
        let sauber = zeile.trim_end_matches(['\r', '\n']);
        if sauber.is_empty() {
            break;
        }
        if let Some((name, wert)) = sauber.split_once(':') {
            if name.trim().eq_ignore_ascii_case("x-norns-bote") {
                gefunden = Some(wert.trim().to_string());
            }
        }
    }
    gefunden
}

fn antworten(mut strom: TcpStream, stand: &Arc<Mutex<Stand>>, wort: &str) -> std::io::Result<()> {
    strom.set_read_timeout(Some(Duration::from_secs(2)))?;
    strom.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut leser = BufReader::new(strom.try_clone()?);
    let mut zeile = String::new();
    (&mut leser).take(2049).read_line(&mut zeile)?;
    if zeile.len() > 2048 || !zeile.ends_with('\n') {
        return abschicken(&mut strom, 400, "{}");
    }
    if zeile.split_whitespace().next() != Some("GET") {
        return abschicken(&mut strom, 405, "{}");
    }
    let weg = zeile.split_whitespace().nth(1).unwrap_or("/").to_string();

    // ⛔ B23: das Wort steht VOR allem. Erst danach wird eine Zahl gelesen.
    let vorgezeigt = wort_aus_kopfzeilen(&mut leser).unwrap_or_default();
    if !wort_stimmt(&vorgezeigt, wort) {
        return abschicken(
            &mut strom,
            401,
            &json!({
                "fehler": "Der Bote kennt dieses Wort nicht. Es steht beim Start des Boten \
            auf dem Prüfgerät und gehört in die Geräteeinstellungen der Kasse."
            })
            .to_string(),
        );
    }

    let route = weg.split('?').next().unwrap_or("");
    if !matches!(route, "/messungen" | "/stand" | "/") {
        return abschicken(&mut strom, 404, "{}");
    }
    let seit = weg
        .split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|p| p.strip_prefix("seit=")))
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(-1);
    let gesperrt = stand.lock().unwrap_or_else(|e| e.into_inner());
    let alter = gesperrt
        .letzte_lesung
        .map(|t| t.elapsed().as_millis() as u64);
    let bereit = gesperrt.fehler.is_none() && alter.is_some_and(|a| a <= 5000);
    let liste: Vec<_> = if bereit {
        gesperrt
            .messungen
            .iter()
            .filter(|m| m.id > seit)
            .map(|m| m.als_json())
            .collect()
    } else {
        Vec::new()
    };
    let koerper = json!({
        "bote":"norns-xrf-agent", "fassung":env!("CARGO_PKG_VERSION"),
        "protokoll":2, "quelleOk":bereit, "alterMs":alter,
        "sitzung":gesperrt.sitzung, "revision":gesperrt.revision,
        "fehler":if bereit { None } else { Some("Die Geraetedaten sind momentan nicht lesbar.") },
        "messungen":liste,
        "anzahl":if bereit { gesperrt.messungen.len() } else { 0 },
    });
    let schluessel = if bereit { 200 } else { 503 };
    drop(gesperrt);

    abschicken(&mut strom, schluessel, &koerper.to_string())
}

/// Eine Antwort hinausgeben.
///
/// ⛔ Ohne `Access-Control-Allow-Origin`: der Fragende ist eine Kasse, kein
/// Browser. Die Kopfzeile stand hier auf `*` und lud jede Webseite ein, die
/// Messungen des Geräts zu lesen.
fn abschicken(strom: &mut TcpStream, schluessel: u16, text: &str) -> std::io::Result<()> {
    let grund = match schluessel {
        200 => "OK",
        401 => "Unauthorized",
        400 => "Bad Request",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        _ => "Not Found",
    };
    let kopf = format!(
        "HTTP/1.1 {schluessel} {grund}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        text.len()
    );
    strom.write_all(kopf.as_bytes())?;
    strom.write_all(text.as_bytes())?;
    strom.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn ein_gleiches_wort_stimmt() {
        assert!(wort_stimmt("abc123", "abc123"));
    }

    #[test]
    fn ein_fremdes_wort_stimmt_nicht() {
        assert!(!wort_stimmt("abc124", "abc123"));
    }

    #[test]
    fn ein_leeres_wort_stimmt_nie() {
        // Genau der Fall vor dem 07.09.2026: gar keine Kopfzeile.
        assert!(!wort_stimmt("", "abc123"));
    }

    #[test]
    fn eine_andere_laenge_stimmt_nicht() {
        assert!(!wort_stimmt("abc", "abc123"));
    }

    #[test]
    fn das_wort_wird_aus_den_kopfzeilen_gelesen() {
        let roh = "Host: 10.0.0.5\r\nX-Norns-Bote: geheim\r\n\r\n";
        let mut leser = Cursor::new(roh.as_bytes());
        assert_eq!(wort_aus_kopfzeilen(&mut leser), Some("geheim".to_string()));
    }

    #[test]
    fn ohne_kopfzeile_kommt_nichts_zurueck() {
        let roh = "Host: 10.0.0.5\r\n\r\n";
        let mut leser = Cursor::new(roh.as_bytes());
        assert_eq!(wort_aus_kopfzeilen(&mut leser), None);
    }
}
