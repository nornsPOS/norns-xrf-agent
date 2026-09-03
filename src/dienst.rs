//! The read-only web interface the point-of-sale system reads from.
//!
//! Written by hand rather than with a web framework: there are two
//! addresses, and every library is one more thing to keep patched on a
//! device meant for measuring.
//!
//! Nothing here accepts input. There is no address that changes anything,
//! and none that starts a measurement.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::messung::Messung;

/// The agent's port, next to the analyser's own 9612 and 9613, so that a
/// system which found the analyser also finds the agent.
pub const PUNKT: u16 = 9614;

pub struct Stand {
    pub messungen: Vec<Messung>,
    pub quelle: String,
    pub fehler: Option<String>,
}

pub fn bedienen(stand: Arc<Mutex<Stand>>) -> Result<(), String> {
    let horcher = TcpListener::bind(("0.0.0.0", PUNKT))
        .map_err(|e| format!("Der Anschlusspunkt {PUNKT} liess sich nicht öffnen: {e}"))?;
    for verbindung in horcher.incoming() {
        let Ok(strom) = verbindung else { continue };
        let stand = Arc::clone(&stand);
        // One thread per request: a stalled connection must not hold up the
        // next measurement.
        std::thread::spawn(move || {
            let _ = antworten(strom, &stand);
        });
    }
    Ok(())
}

fn antworten(mut strom: TcpStream, stand: &Arc<Mutex<Stand>>) -> std::io::Result<()> {
    let mut leser = BufReader::new(strom.try_clone()?);
    let mut zeile = String::new();
    leser.read_line(&mut zeile)?;
    let weg = zeile.split_whitespace().nth(1).unwrap_or("/").to_string();

    let (schluessel, koerper) = if weg.starts_with("/messungen") {
        let seit = weg
            .split_once("seit=")
            .and_then(|(_, r)| r.split(['&', ' ']).next())
            .and_then(|z| z.parse::<i64>().ok())
            .unwrap_or(-1);
        let gesperrt = stand.lock().unwrap_or_else(|e| e.into_inner());
        let liste: Vec<_> = gesperrt
            .messungen
            .iter()
            .filter(|m| m.id > seit)
            .map(|m| m.als_json())
            .collect();
        (200, json!({ "messungen": liste }))
    } else if weg == "/" || weg.starts_with("/stand") {
        let gesperrt = stand.lock().unwrap_or_else(|e| e.into_inner());
        (
            200,
            json!({
                "bote": "norns-xrf-agent",
                "fassung": env!("CARGO_PKG_VERSION"),
                "quelle": gesperrt.quelle,
                "fehler": gesperrt.fehler,
                "letzte": gesperrt.messungen.last().map(|m| m.als_json()),
                "anzahl": gesperrt.messungen.len(),
            }),
        )
    } else {
        (404, json!({ "fehler": "Diese Adresse kennt der Bote nicht." }))
    };

    let text = koerper.to_string();
    let kopf = format!(
        "HTTP/1.1 {schluessel} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        if schluessel == 200 { "OK" } else { "Not Found" },
        text.len()
    );
    strom.write_all(kopf.as_bytes())?;
    strom.write_all(text.as_bytes())?;
    strom.flush()
}
