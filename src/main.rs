//! Norns XRF Agent.
//!
//! Reads finished measurements from an XRF analyser and offers them,
//! read-only, on the local network.
//!
//! The analyser publishes its identity, state and raw spectrum openly on TCP
//! 9612 and 9613, but not the calculated concentrations: those are written by
//! the manufacturer software into a local database. This agent closes that
//! gap and nothing else.
//!
//! Analysers of this kind have a touch screen and no keyboard, so opening the
//! executable with no arguments installs it. Administrator rights are
//! requested at that point, and the result is shown in a window.
//!
//! ```text
//! norns-xrf-agent                installs, starts, reports
//! norns-xrf-agent --entfernen    removes the task and the firewall rule
//! norns-xrf-agent --jetzt [path] runs in the foreground
//! norns-xrf-agent --probe [path] prints the five most recent measurements
//! ```
//!
//! The analyser contains an X-ray tube. The agent is read-only: no write
//! statement, no address that changes anything, and none that starts a
//! measurement.

mod datenbank;
mod dienst;
mod messung;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// How many measurements are kept in memory.
const VORRAT: usize = 200;

/// Name of the scheduled task that starts the agent with the machine.
#[cfg(windows)]
const AUFGABE: &str = "Norns XRF Agent";

/// How often the database is checked for new measurements.
const TAKT: std::time::Duration = std::time::Duration::from_millis(700);

fn main() {
    let argumente: Vec<String> = std::env::args().collect();
    // With no argument the agent installs itself: on a device without a
    // keyboard, opening the file is the only invocation there will ever be.
    let befehl = argumente.get(1).map(String::as_str).unwrap_or("--einrichten");
    let pfad = argumente.get(2).map(PathBuf::from);

    match befehl {
        // The rights are not guessed at, they are attempted. If Windows
        // refuses, the agent relaunches itself asking for them. The marker
        // argument keeps that from becoming a loop.
        "--einrichten" | "--erhoeht" => {
            match einrichten(pfad.clone()) {
            Ok(satz) => {
                println!("{satz}");
                melden("Norns XRF Agent", &satz, false);
            }
            Err(grund) => {
                let verweigert = grund.contains("FAILED 5") || grund.contains("denied");
                if verweigert && befehl != "--erhoeht" && erhoehen(pfad.as_deref()) {
                    // The elevated instance has taken over.
                    return;
                }
                let satz = format!("Nicht eingerichtet.\n\n{grund}");
                eprintln!("{satz}");
                melden("Norns XRF Agent", &satz, true);
                std::process::exit(1);
            }
            }
        }
        "--entfernen" => match entfernen() {
            Ok(satz) => println!("{satz}"),
            Err(grund) => {
                eprintln!("Nicht entfernt: {grund}");
                std::process::exit(1);
            }
        },
        "--probe" => match probe(pfad) {
            Ok(satz) => println!("{satz}"),
            Err(grund) => {
                eprintln!("{grund}");
                std::process::exit(1);
            }
        },
        _ => {
            if let Err(grund) = laufen(pfad) {
                eprintln!("Der Bote hat aufgegeben: {grund}");
                std::process::exit(1);
            }
        }
    }
}

/// Relaunches the agent asking Windows for administrator rights. Returns
/// true when the second instance has been started.
#[cfg(windows)]
fn erhoehen(pfad: Option<&std::path::Path>) -> bool {
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            fenster: isize,
            verb: *const u16,
            datei: *const u16,
            argumente: *const u16,
            ordner: *const u16,
            zeigen: i32,
        ) -> isize;
    }
    const NORMAL: i32 = 1;
    let Ok(ich) = std::env::current_exe() else {
        return false;
    };
    let breit = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let verb = breit("runas");
    let datei = breit(&ich.to_string_lossy());
    // The marker means this is already the second attempt.
    let argumente = match pfad {
        Some(p) => breit(&format!("--erhoeht \"{}\"", p.display())),
        None => breit("--erhoeht"),
    };
    // Above 32 means Windows accepted the call.
    let aus = unsafe {
        ShellExecuteW(
            0,
            verb.as_ptr(),
            datei.as_ptr(),
            argumente.as_ptr(),
            std::ptr::null(),
            NORMAL,
        )
    };
    aus > 32
}

#[cfg(not(windows))]
fn erhoehen(_pfad: Option<&std::path::Path>) -> bool {
    false
}

/// Shows the result in a window that can be dismissed by touch. A single
/// call to user32; no library is needed for it.
#[cfg(windows)]
fn melden(titel: &str, text: &str, fehler: bool) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(fenster: isize, text: *const u16, titel: *const u16, art: u32) -> i32;
    }
    const INFORMATION: u32 = 0x0000_0040;
    const WARNUNG: u32 = 0x0000_0030;
    const VORDERGRUND: u32 = 0x0001_0000;
    let breit = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (t, k) = (breit(text), breit(titel));
    unsafe {
        MessageBoxW(
            0,
            t.as_ptr(),
            k.as_ptr(),
            (if fehler { WARNUNG } else { INFORMATION }) | VORDERGRUND,
        );
    }
}

#[cfg(not(windows))]
fn melden(_titel: &str, _text: &str, _fehler: bool) {}

/// The database: given, remembered, or located.
fn quelle_finden(gesagt: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(p) = gesagt {
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("Dort liegt keine Datenbank: {}", p.display()))
        };
    }
    if let Some(gemerkt) = gemerkter_pfad() {
        if gemerkt.is_file() {
            return Ok(gemerkt);
        }
    }
    datenbank::finden().ok_or_else(|| {
        "Die Datenbank des Geräts wurde nicht gefunden. Bitte den Pfad zu \
`samplesummary.db` als zweites Wort mitgeben."
            .to_string()
    })
}

fn arbeitsort() -> PathBuf {
    std::env::temp_dir().join("norns-xrf-agent")
}

fn merkzettel() -> PathBuf {
    arbeitsort().join("quelle.txt")
}

fn gemerkter_pfad() -> Option<PathBuf> {
    std::fs::read_to_string(merkzettel())
        .ok()
        .map(|s| PathBuf::from(s.trim()))
}

fn pfad_merken(p: &std::path::Path) {
    let _ = std::fs::create_dir_all(arbeitsort());
    let _ = std::fs::write(merkzettel(), p.to_string_lossy().as_bytes());
}

/// Reads once and prints, so a technician can confirm the agent finds the
/// analyser before installing anything.
fn probe(gesagt: Option<PathBuf>) -> Result<String, String> {
    let pfad = quelle_finden(gesagt)?;
    let _ = std::fs::create_dir_all(arbeitsort());
    let quelle = datenbank::Quelle::neu(pfad.clone(), &arbeitsort());
    let messungen = quelle.messungen_seit(-1, 10_000)?;
    let mut aus = format!(
        "Datenbank: {}\nMessungen insgesamt: {}\n",
        pfad.display(),
        messungen.len()
    );
    for m in messungen.iter().rev().take(5) {
        let karat = m
            .karat()
            .map(|k| format!("{k:.1} Karat"))
            .unwrap_or_else(|| "kein Gold".into());
        aus.push_str(&format!(
            "  #{:<6} {:<12} {:<18} {karat}\n",
            m.id, m.anwendung, m.probe
        ));
    }
    Ok(aus)
}

/// The loop: watch, read, keep available.
fn laufen(gesagt: Option<PathBuf>) -> Result<(), String> {
    let pfad = quelle_finden(gesagt)?;
    pfad_merken(&pfad);
    let _ = std::fs::create_dir_all(arbeitsort());
    let quelle = datenbank::Quelle::neu(pfad.clone(), &arbeitsort());

    let stand = Arc::new(Mutex::new(dienst::Stand {
        messungen: Vec::new(),
        quelle: pfad.to_string_lossy().to_string(),
        fehler: None,
    }));

    // The web interface in its own thread; the watching happens here.
    {
        let stand = Arc::clone(&stand);
        std::thread::spawn(move || {
            if let Err(grund) = dienst::bedienen(stand) {
                eprintln!("{grund}");
            }
        });
    }

    let mut letzte_id: i64 = -1;
    let mut letzter_stand: Option<(u64, i64)> = None;
    loop {
        let jetzt = quelle.stand();
        // Read only when the file has actually changed.
        if jetzt != letzter_stand {
            letzter_stand = jetzt;
            match quelle.messungen_seit(letzte_id, VORRAT) {
                Ok(neue) => {
                    if !neue.is_empty() {
                        letzte_id = neue.last().map(|m| m.id).unwrap_or(letzte_id);
                        let mut gesperrt = stand.lock().unwrap_or_else(|e| e.into_inner());
                        gesperrt.messungen.extend(neue);
                        let ueberzaehlig = gesperrt.messungen.len().saturating_sub(VORRAT);
                        gesperrt.messungen.drain(..ueberzaehlig);
                        gesperrt.fehler = None;
                    }
                }
                Err(grund) => {
                    let mut gesperrt = stand.lock().unwrap_or_else(|e| e.into_inner());
                    gesperrt.fehler = Some(grund);
                }
            }
        }
        std::thread::sleep(TAKT);
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Installation
// ─────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
fn einrichten(gesagt: Option<PathBuf>) -> Result<String, String> {
    use std::process::Command;
    let pfad = quelle_finden(gesagt)?;
    pfad_merken(&pfad);
    let hier = std::env::current_exe()
        .map_err(|e| format!("Der Bote findet sich selbst nicht: {e}"))?;

    // Stop an older copy first: it would still hold the port, and the new
    // instance could not open it.
    let _ = Command::new("schtasks").args(["/end", "/tn", AUFGABE]).output();
    let eigene = std::process::id();
    let _ = Command::new("taskkill")
        .args([
            "/F",
            "/IM",
            "norns-xrf-agent.exe",
            "/FI",
            &format!("PID ne {eigene}"),
        ])
        .output();

    // Move to a fixed location, so the task does not depend on a file in
    // the Downloads folder.
    let heim = std::path::PathBuf::from(
        std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into()),
    )
    .join("Norns");
    let _ = std::fs::create_dir_all(&heim);
    let ich = heim.join("norns-xrf-agent.exe");
    // Skip when already in place.
    if hier != ich {
        std::fs::copy(&hier, &ich).map_err(|e| {
            format!("Der Bote liess sich nicht nach {} legen: {e}", heim.display())
        })?;
    }

    // A scheduled task rather than a Windows service: a service has to
    // report to the service control manager within thirty seconds, which an
    // ordinary program cannot do. The task starts with the machine, runs as
    // SYSTEM, and needs nobody logged in.
    let ruf = format!("\"{}\" --jetzt \"{}\"", ich.display(), pfad.display());
    let anlegen = Command::new("schtasks")
        .args([
            "/create",
            "/tn",
            AUFGABE,
            "/tr",
            &ruf,
            "/sc",
            "onstart",
            "/ru",
            "SYSTEM",
            "/rl",
            "HIGHEST",
            "/f",
        ])
        .output()
        .map_err(|e| format!("`schtasks` liess sich nicht rufen: {e}"))?;
    if !anlegen.status.success() {
        let text = String::from_utf8_lossy(&anlegen.stdout);
        let fehler = String::from_utf8_lossy(&anlegen.stderr);
        return Err(format!(
            "Die Aufgabe liess sich nicht anlegen: {} {}",
            text.trim(),
            fehler.trim()
        ));
    }

    // Open the firewall for the local subnet only.
    let _ = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            "name=Norns XRF Agent",
            "dir=in",
            "action=allow",
            "protocol=TCP",
            &format!("localport={}", dienst::PUNKT),
            "profile=any",
            "remoteip=localsubnet",
        ])
        .output();

    let _ = Command::new("schtasks").args(["/run", "/tn", AUFGABE]).output();
    Ok(format!(
        "Der Bote ist eingerichtet und laeuft.\n\n  Datenbank:      {}\n  Anschlusspunkt: {}\n  Aufgabe:        {} (startet mit dem Rechner)",
        pfad.display(),
        dienst::PUNKT,
        AUFGABE
    ))
}

#[cfg(not(windows))]
fn einrichten(_gesagt: Option<PathBuf>) -> Result<String, String> {
    Err("Als Dienst richtet sich der Bote nur unter Windows ein; das Gerät \
läuft mit Windows. Zum Ausprobieren hier: `--jetzt`."
        .to_string())
}

#[cfg(windows)]
fn entfernen() -> Result<String, String> {
    use std::process::Command;
    let _ = Command::new("schtasks").args(["/end", "/tn", AUFGABE]).output();
    let aus = Command::new("schtasks")
        .args(["/delete", "/tn", AUFGABE, "/f"])
        .output()
        .map_err(|e| format!("`schtasks` liess sich nicht rufen: {e}"))?;
    let _ = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            "name=Norns XRF Agent",
        ])
        .output();
    if aus.status.success() {
        Ok("Der Bote ist ausgetragen.".to_string())
    } else {
        Err(String::from_utf8_lossy(&aus.stdout).trim().to_string())
    }
}

#[cfg(not(windows))]
fn entfernen() -> Result<String, String> {
    Err("Nur unter Windows.".to_string())
}
