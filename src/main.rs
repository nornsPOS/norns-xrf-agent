//! Liest fertige Herstellerergebnisse. Kein Messstart und kein Schreibzugriff
//! auf die Herstellerdatenbank. Derselbe Quelltext wird separat ausgeliefert.
mod datenbank;
mod dienst;
mod einrichtung;
mod messung;
mod speicher;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
const VORRAT: usize = 200;
const TAKT: Duration = Duration::from_millis(700);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let befehl = args.get(1).map(String::as_str).unwrap_or("--einrichten");
    let pfad = args.get(2).map(PathBuf::from);
    if befehl == "--einrichten-still" {
        std::env::set_var("NORNS_XRF_SILENT", "1");
    }
    let aus = match befehl {
        "--einrichten-still" => einrichtung::installieren(pfad, true),
        "--einrichten" | "--erhoeht" => einrichtung::installieren(pfad, befehl == "--erhoeht"),
        "--entfernen" => einrichtung::entfernen(),
        "--probe" => probe(pfad),
        "--jetzt" => laufen(pfad),
        _ => Err(
            "Unbekannter Aufruf. Bitte --einrichten, --jetzt, --probe oder --entfernen verwenden."
                .into(),
        ),
    };
    if let Err(grund) = aus {
        eprintln!("{grund}");
        if matches!(befehl, "--einrichten" | "--erhoeht") {
            einrichtung::melden(&grund, true);
        }
        std::process::exit(1);
    }
}

fn quelle_finden(gesagt: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(p) = gesagt {
        return Ok(p);
    }
    // Einen voruebergehend fehlenden konfigurierten Pfad NICHT durch eine
    // zufaellige historische Datenbank ersetzen. Der Leser versucht ihn erneut.
    if let Ok(p) = std::fs::read_to_string(speicher::ort()?.join("quelle.txt")) {
        if !p.trim().is_empty() {
            return Ok(PathBuf::from(p.trim()));
        }
    }
    datenbank::suchen().ok_or_else(|| {
        "Die Datenbank wurde nicht gefunden. Bitte den Pfad nach --einrichten angeben.".into()
    })
}

fn probe(pfad: Option<PathBuf>) -> Result<(), String> {
    let quelle = datenbank::Quelle::neu(quelle_finden(pfad)?, &speicher::ort()?);
    let messungen = quelle.messungen_seit(-1, VORRAT)?;
    println!("Die letzten {} Ergebnisse wurden gelesen.", messungen.len());
    for m in messungen.iter().rev().take(5) {
        println!("Messung {}: {:?} Promille Gold", m.id, m.gold_promille());
    }
    Ok(())
}

fn laufen(pfad: Option<PathBuf>) -> Result<(), String> {
    let ort = speicher::ort()?;
    let pfad = quelle_finden(pfad)?;
    let wort = speicher::wort(&ort)?;
    let quelle = datenbank::Quelle::neu(pfad.clone(), &ort);
    let adresse = std::env::var("NORNS_XRF_LISTEN").unwrap_or_else(|_| "0.0.0.0:9614".into());
    // Vor dem Leser binden: ein belegter Port beendet den Prozess mit Fehler.
    let horcher = std::net::TcpListener::bind(&adresse)
        .map_err(|e| format!("Der Ergebnisanschluss konnte nicht geoeffnet werden: {e}"))?;
    let stand = Arc::new(Mutex::new(dienst::Stand {
        messungen: Vec::new(),
        fehler: Some("Die erste Quellenpruefung steht noch aus.".into()),
        letzte_lesung: None,
        sitzung: speicher::zufall()?,
        revision: 0,
    }));
    let lesestand = Arc::clone(&stand);
    std::thread::spawn(move || loop {
        let gelesen = quelle.messungen_seit(-1, VORRAT);
        let mut stand = lesestand.lock().unwrap_or_else(|e| e.into_inner());
        match gelesen {
            Ok(neue) => {
                if stand.messungen != neue {
                    stand.revision = stand.revision.wrapping_add(1);
                }
                // Vollstaendigen aktuellen Ausschnitt ersetzen: gleiche Kennung
                // darf nachtraeglich gefuellt sein, IDs duerfen neu beginnen.
                stand.messungen = neue;
                stand.fehler = None;
                stand.letzte_lesung = Some(Instant::now());
            }
            Err(grund) => {
                stand.messungen.clear();
                stand.fehler = Some(grund);
            }
        }
        drop(stand);
        std::thread::sleep(TAKT);
    });
    dienst::bedienen(horcher, stand, wort)
}
