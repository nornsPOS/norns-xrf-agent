//! Reading the analyser's measurement database.
//!
//! Read-only throughout: there is no write statement in this file.

use std::path::{Path, PathBuf};

use crate::messung::{elemente_lesen, Messung};

/// The database, located automatically or given explicitly.
pub struct Quelle {
    pub datei: PathBuf,
    abschrift: PathBuf,
}

impl Quelle {
    pub fn neu(datei: PathBuf, arbeitsort: &Path) -> Self {
        Self {
            datei,
            abschrift: arbeitsort.join("messungen-abschrift.db"),
        }
    }

    /// Opens the database read-only.
    ///
    /// A read-only connection does not disturb the manufacturer software
    /// writing to the same file; SQLite is built for a writer and readers to
    /// coexist. If the file is locked, a copy is read instead. When both
    /// fail, the error names both reasons.
    fn oeffnen(&self) -> Result<rusqlite::Connection, String> {
        let nur_lesen = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_URI;
        let pfad = self.datei.to_string_lossy().replace('\\', "/");

        // Direct, read-only.
        let direkt = rusqlite::Connection::open_with_flags(
            format!("file:{pfad}?mode=ro"),
            nur_lesen,
        );
        let grund_direkt = match direkt {
            Ok(v) => return Ok(v),
            Err(e) => e.to_string(),
        };

        // Immutable, for the case that SQLite may not write its side files.
        if let Ok(v) = rusqlite::Connection::open_with_flags(
            format!("file:{pfad}?mode=ro&immutable=1"),
            nur_lesen,
        ) {
            return Ok(v);
        }

        // Last resort: read a copy.
        let grund_abschrift = match std::fs::copy(&self.datei, &self.abschrift) {
            Ok(_) => {
                return rusqlite::Connection::open_with_flags(
                    &self.abschrift,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .map_err(|e| format!("Die Abschrift liess sich nicht öffnen: {e}"))
            }
            Err(e) => e.to_string(),
        };

        Err(format!(
            "Die Datenbank des Geräts liess sich nicht lesen. Direkt: {grund_direkt}. \
Als Abschrift: {grund_abschrift}."
        ))
    }

    /// Size and modification time. A change means the analyser has measured.
    pub fn stand(&self) -> Option<(u64, i64)> {
        let m = std::fs::metadata(&self.datei).ok()?;
        let zeit = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Some((m.len(), zeit))
    }

    /// Every measurement after `seit_id`, most recent last.
    pub fn messungen_seit(&self, seit_id: i64, hoechstens: usize) -> Result<Vec<Messung>, String> {
        let verbindung = self.oeffnen()?;

        let mut satz = verbindung
            .prepare(
                "SELECT KeyId, AppName, SampleName, MeasureTime, InfoSaveFile, ResultContent
                   FROM summarys
                  WHERE KeyId > ?1
                  ORDER BY KeyId
                  LIMIT ?2",
            )
            .map_err(|e| format!("Die Tafel `summarys` fehlt: {e}"))?;

        let zeilen = satz
            .query_map(rusqlite::params![seit_id, hoechstens as i64], |z| {
                let inhalt: String = z.get(5).unwrap_or_default();
                Ok(Messung {
                    id: z.get(0)?,
                    anwendung: z.get::<_, String>(1).unwrap_or_default(),
                    probe: z.get::<_, String>(2).unwrap_or_default(),
                    gemessen_am: z.get::<_, i64>(3).unwrap_or(0),
                    datei: z.get::<_, String>(4).unwrap_or_default(),
                    elemente: elemente_lesen(&inhalt),
                })
            })
            .map_err(|e| format!("Die Messungen liessen sich nicht lesen: {e}"))?;

        let mut aus = Vec::new();
        for z in zeilen {
            match z {
                Ok(m) => aus.push(m),
                // One broken row does not discard the rest.
                Err(_) => continue,
            }
        }
        Ok(aus)
    }

}

/// Locates the database by asking the machine rather than searching it.
///
/// The process listening on the analyser's own port 9612 is its measurement
/// engine. Its executable path leads to the installation, and the database
/// sits a few folders above it. If that software is not running, a short
/// list of known locations is checked.
pub fn finden() -> Option<PathBuf> {
    ueber_den_horcher().or_else(ueber_die_ueblichen_stellen)
}

/// The port the analyser's measurement engine listens on.
#[cfg(windows)]
const PUNKT_DES_GERAETS: &str = ":9612";

/// Vom horchenden Programm zu seinem Ordner, und von dort zur Datenbank.
#[cfg(windows)]
fn ueber_den_horcher() -> Option<PathBuf> {
    use std::process::Command;
    // Get-NetTCPConnection reports the state as a value rather than as a
    // localised word, so nothing has to be translated or matched by text.
    let frage = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$p = (Get-NetTCPConnection -LocalPort 9612 -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1).OwningProcess; if ($p) { (Get-Process -Id $p -ErrorAction SilentlyContinue).Path }",
        ])
        .output()
        .ok()?;
    let programm = String::from_utf8_lossy(&frage.stdout).trim().to_string();
    if !programm.is_empty() {
        if let Some(gefunden) = von_dort_aufwaerts(&PathBuf::from(&programm)) {
            return Some(gefunden);
        }
    }

    // Fallback for Windows versions without Get-NetTCPConnection. Matched on
    // the address only, never on a localised state word.
    let netz = Command::new("netstat").args(["-ano", "-p", "TCP"]).output().ok()?;
    let text = String::from_utf8_lossy(&netz.stdout);
    let kennung: u32 = text
        .lines()
        .filter(|z| {
            z.split_whitespace()
                .nth(1)
                .is_some_and(|anschrift| anschrift.ends_with(PUNKT_DES_GERAETS))
        })
        .filter_map(|z| z.split_whitespace().last())
        .find_map(|w| w.parse().ok())?;
    let frage = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-Process -Id {kennung} -ErrorAction SilentlyContinue).Path"),
        ])
        .output()
        .ok()?;
    von_dort_aufwaerts(&PathBuf::from(
        String::from_utf8_lossy(&frage.stdout).trim(),
    ))
}

#[cfg(not(windows))]
fn ueber_den_horcher() -> Option<PathBuf> {
    None
}

/// Vom Programm des Geräts aus nach oben gehen und auf jeder Ebene fragen,
/// ob hier `Data\User\samplesummary.db` liegt. Mehr als ein paar Ebenen
/// braucht es nie: die Datenbank gehört zur Software, nicht zur Platte.
#[cfg(windows)]
fn von_dort_aufwaerts(programm: &Path) -> Option<PathBuf> {
    let mut ort = programm.parent()?;
    for _ in 0..6 {
        let treffer = ort.join("Data").join("User").join(NAME);
        if treffer.is_file() {
            return Some(treffer);
        }
        ort = ort.parent()?;
    }
    None
}

/// The database file name used by this manufacturer.
const NAME: &str = "samplesummary.db";

/// Known locations, used when the analyser software is not running.
fn ueber_die_ueblichen_stellen() -> Option<PathBuf> {
    let mut kandidaten: Vec<PathBuf> = Vec::new();
    for laufwerk in ["C:\\", "D:\\"] {
        for zweig in [
            "",
            "Pureray",
            "XRF",
            "Xrf",
            "Program Files",
            "Program Files (x86)",
        ] {
            let wurzel = if zweig.is_empty() {
                PathBuf::from(laufwerk)
            } else {
                PathBuf::from(laufwerk).join(zweig)
            };
            kandidaten.push(wurzel.join("Data").join("User").join(NAME));
        }
    }
    // And beside the agent itself.
    if let Ok(ich) = std::env::current_exe() {
        if let Some(ordner) = ich.parent() {
            kandidaten.push(ordner.join("Data").join("User").join(NAME));
            kandidaten.push(ordner.join(NAME));
        }
    }
    kandidaten.into_iter().find(|p| p.is_file())
}
