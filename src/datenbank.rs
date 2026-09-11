//! Die Datenbank des Geräts lesen, ohne sie zu stören.
//!
//! SQLite wird ausschliesslich lesend geoeffnet. Der SELECT sieht einen
//! konsistenten Stand einschliesslich des WAL. Eine Dateikopie allein liess
//! neue Messungen im WAL zurueck oder kopierte einen halb geschriebenen Stand.
//! Die kurze Sperrfrist begrenzt das Warten auf die Herstellersoftware.
//!
//! Gelesen wird NUR. Der Bote hat keinen einzigen schreibenden Befehl.

use std::path::{Path, PathBuf};

use crate::messung::{elemente_lesen, Messung};

/// Wo die Datenbank des Geräts liegt, gefunden oder gesagt bekommen.
pub struct Quelle {
    pub datei: PathBuf,
}

impl Quelle {
    pub fn neu(datei: PathBuf, _arbeitsort: &Path) -> Self {
        Self { datei }
    }

    /// Alle Messungen NACH `seit_id`, jüngste zuletzt.
    /// Bei negativem `seit_id` nur den zusammenhaengenden gueltigen neuesten
    /// Stapel lesen; ein defektes aelteres Ergebnis bildet dessen Grenze.
    pub fn messungen_seit(&self, seit_id: i64, hoechstens: usize) -> Result<Vec<Messung>, String> {
        let verbindung = rusqlite::Connection::open_with_flags(
            &self.datei,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| format!("Die Geraetedaten liessen sich nicht lesend oeffnen: {e}"))?;
        verbindung
            .busy_timeout(std::time::Duration::from_millis(250))
            .map_err(|e| format!("Die Lesefrist liess sich nicht setzen: {e}"))?;

        // Erster Anschluss: neueste Ergebnisse, kein jahrzehntealter Anfang.
        // Danach nach Kennung weiterlesen, begrenzt und ohne Luecken im Stapel.
        let abfrage = if seit_id < 0 {
            "SELECT KeyId, AppName, SampleName, MeasureTime, InfoSaveFile, ResultContent FROM summarys WHERE KeyId > ?1 ORDER BY KeyId DESC LIMIT ?2"
        } else {
            "SELECT KeyId, AppName, SampleName, MeasureTime, InfoSaveFile, ResultContent FROM summarys WHERE KeyId > ?1 ORDER BY KeyId LIMIT ?2"
        };

        let mut satz = verbindung
            .prepare(abfrage)
            .map_err(|e| format!("Die Tafel `summarys` fehlt: {e}"))?;

        let zeilen = satz
            .query_map(rusqlite::params![seit_id, hoechstens as i64], |z| {
                let inhalt: String = z.get(5)?;
                Ok(Messung {
                    id: z.get(0)?,
                    anwendung: z.get::<_, String>(1).unwrap_or_default(),
                    probe: z.get::<_, String>(2).unwrap_or_default(),
                    gemessen_am: z.get::<_, i64>(3)?,
                    datei: z.get::<_, String>(4).unwrap_or_default(),
                    elemente: elemente_lesen(&inhalt).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Text,
                            std::io::Error::new(std::io::ErrorKind::InvalidData, e).into(),
                        )
                    })?,
                })
            })
            .map_err(|e| format!("Die Messungen liessen sich nicht lesen: {e}"))?;

        let mut aus = Vec::new();
        for z in zeilen {
            match z {
                Ok(m) => aus.push(m),
                // Alte abgebrochene Messungen duerfen eine neuere vollstaendige
                // Messung nicht sperren. Nur der Ergebnisinhalt darf diese
                // Grenze bilden, nie ein Datenbank- oder Abfragefehler.
                Err(
                    rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, _)
                    | rusqlite::Error::InvalidColumnType(5, _, _),
                ) if seit_id < 0 && !aus.is_empty() => {
                    break;
                }
                // Kein stiller Rueckfall auf eine aeltere gueltige Messung.
                Err(e) => {
                    return Err(format!(
                        "Eine Geraetemessung ist noch unvollstaendig oder unlesbar: {e}"
                    ))
                }
            }
        }
        if seit_id < 0 {
            aus.reverse();
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
pub fn suchen() -> Option<PathBuf> {
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
    let netz = Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
        .ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Probe {
        ort: PathBuf,
        db: Option<rusqlite::Connection>,
        quelle: Quelle,
    }
    impl Probe {
        fn neu(wal: bool) -> Self {
            let mut zufall = [0u8; 8];
            getrandom::getrandom(&mut zufall).unwrap();
            let ort =
                std::env::temp_dir().join(format!("norns-xrf-{}", u64::from_ne_bytes(zufall)));
            std::fs::create_dir(&ort).unwrap();
            let datei = ort.join("samplesummary.db");
            let db = rusqlite::Connection::open(&datei).unwrap();
            db.execute_batch("CREATE TABLE summarys (KeyId INTEGER PRIMARY KEY, AppName TEXT, SampleName TEXT, MeasureTime INTEGER, InfoSaveFile TEXT, ResultContent TEXT);").unwrap();
            if wal {
                db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
                    .unwrap();
            }
            let quelle = Quelle::neu(datei, &ort);
            Self {
                ort,
                db: Some(db),
                quelle,
            }
        }
        fn einfuegen(&self, id: i64) {
            self.db
                .as_ref()
                .unwrap()
                .execute(
                    "INSERT INTO summarys VALUES (?1, 'Metall', 'Probe', 1780000000, '', '<Result><Compo Name=\"Au\" Fractal=\"58.5\" /></Result>')",
                    [id],
                )
                .unwrap();
        }
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            drop(self.db.take());
            let _ = std::fs::remove_dir_all(&self.ort);
        }
    }

    #[test]
    fn der_ersteintritt_sieht_die_neuesten_statt_der_aeltesten_zweihundert() {
        let p = Probe::neu(false);
        p.db.as_ref().unwrap().execute_batch("BEGIN").unwrap();
        for id in 1..=501 {
            p.einfuegen(id);
        }
        p.db.as_ref().unwrap().execute_batch("COMMIT").unwrap();
        let neu = p.quelle.messungen_seit(-1, 200).unwrap();
        assert_eq!(neu.len(), 200);
        assert_eq!(neu.first().unwrap().id, 302);
        assert_eq!(neu.last().unwrap().id, 501);
        p.einfuegen(502);
        let danach = p.quelle.messungen_seit(501, 200).unwrap();
        assert_eq!(danach.len(), 1);
        assert_eq!(danach[0].id, 502);
    }

    #[test]
    fn alte_unvollstaendige_ergebnisse_begrenzen_nur_den_neuesten_gueltigen_stapel() {
        let p = Probe::neu(true);
        for id in 1..=5 {
            p.einfuegen(id);
        }
        for defekt in ["''", "'<Result />'", "NULL", "X'00'"] {
            p.db.as_ref()
                .unwrap()
                .execute_batch(&format!(
                    "UPDATE summarys SET ResultContent = {defekt} WHERE KeyId = 3"
                ))
                .unwrap();
            let neu = p.quelle.messungen_seit(-1, 200).unwrap();
            assert_eq!(neu.iter().map(|m| m.id).collect::<Vec<_>>(), [4, 5]);
            // Inkrementelles Lesen darf eine Luecke nicht als Erfolg melden.
            assert!(p.quelle.messungen_seit(1, 200).is_err());
        }
        p.db.as_ref()
            .unwrap()
            .execute_batch("DELETE FROM summarys WHERE KeyId = 3")
            .unwrap();
        p.einfuegen(3);
        let repariert = p.quelle.messungen_seit(-1, 200).unwrap();
        assert_eq!(
            repariert.iter().map(|m| m.id).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn neuester_defekt_und_fehler_ausserhalb_des_ergebnisinhalts_bleiben_fehler() {
        let p = Probe::neu(false);
        p.einfuegen(1);
        p.einfuegen(2);
        p.db.as_ref()
            .unwrap()
            .execute_batch("UPDATE summarys SET ResultContent = '' WHERE KeyId = 2")
            .unwrap();
        assert!(p.quelle.messungen_seit(-1, 200).is_err());
        p.db.as_ref()
            .unwrap()
            .execute_batch("DELETE FROM summarys WHERE KeyId = 2")
            .unwrap();
        p.einfuegen(2);
        p.db.as_ref()
            .unwrap()
            .execute_batch("UPDATE summarys SET MeasureTime = 'ungueltig' WHERE KeyId = 1")
            .unwrap();
        assert!(p.quelle.messungen_seit(-1, 200).is_err());
        p.db.as_ref()
            .unwrap()
            .execute_batch("ALTER TABLE summarys RENAME TO andere_tafel")
            .unwrap();
        assert!(p.quelle.messungen_seit(-1, 200).is_err());
    }

    #[test]
    fn ein_commit_im_wal_wird_erkannt_und_gelesen_ohne_checkpoint() {
        let p = Probe::neu(true);
        p.einfuegen(1);
        let neu = p.quelle.messungen_seit(-1, 200).unwrap();
        assert_eq!(neu.len(), 1);
        assert_eq!(neu[0].id, 1);
    }
}
