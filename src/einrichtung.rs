//! Windows-Start als geplante Aufgabe; kein vorgetaeuschter Systemdienst.
#[cfg(windows)]
use std::{path::PathBuf, process::Command};

#[cfg(windows)]
const AUFGABE: &str = "Norns XRF Agent";

#[cfg(windows)]
fn ausfuehren(programm: &str, args: &[&str]) -> Result<(), String> {
    let aus = Command::new(programm)
        .args(args)
        .output()
        .map_err(|e| format!("{programm} konnte nicht gestartet werden: {e}"))?;
    if aus.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{programm} hat die Einrichtung abgelehnt: {} {}",
            String::from_utf8_lossy(&aus.stdout).trim(),
            String::from_utf8_lossy(&aus.stderr).trim()
        ))
    }
}

#[cfg(windows)]
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(windows)]
pub fn installieren(pfad: Option<PathBuf>, bereits_erhoeht: bool) -> Result<(), String> {
    #[link(name = "shell32")]
    extern "system" {
        fn IsUserAnAdmin() -> i32;
        fn ShellExecuteW(
            hwnd: isize,
            op: *const u16,
            file: *const u16,
            args: *const u16,
            dir: *const u16,
            show: i32,
        ) -> isize;
    }
    if unsafe { IsUserAnAdmin() } == 0 {
        if bereits_erhoeht {
            return Err("Die Einrichtung benoetigt Windows-Administratorrechte.".into());
        }
        let ich = std::env::current_exe().map_err(|e| e.to_string())?;
        let args = match pfad {
            Some(ref p) => format!("--erhoeht \"{}\"", p.display()),
            None => "--erhoeht".into(),
        };
        let breit = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let (op, file, args) = (breit("runas"), breit(&ich.to_string_lossy()), breit(&args));
        let code = unsafe {
            ShellExecuteW(
                0,
                op.as_ptr(),
                file.as_ptr(),
                args.as_ptr(),
                std::ptr::null(),
                1,
            )
        };
        return if code > 32 {
            Ok(())
        } else {
            Err("Die Einrichtung wurde nicht freigegeben.".into())
        };
    }
    let quelle = crate::quelle_finden(pfad)?;
    let ort = crate::speicher::ort()?;
    std::fs::create_dir_all(&ort).map_err(|e| e.to_string())?;
    // Gleicher Ort fuer Administrator und SYSTEM; kein Code in einem Temp-Verzeichnis.
    ausfuehren(
        "icacls",
        &[
            ort.to_str().ok_or("Der Programmpfad ist unlesbar.")?,
            "/inheritance:r",
            "/grant:r",
            "*S-1-5-18:(OI)(CI)F",
            "*S-1-5-32-544:(OI)(CI)F",
        ],
    )?;
    let wort = crate::speicher::wort(&ort)?;
    crate::datenbank::Quelle::neu(quelle.clone(), &ort).messungen_seit(-1, 200)?;
    let ich = std::env::current_exe().map_err(|e| e.to_string())?;
    let ziel = ort.join("norns-xrf-agent.exe");
    if Command::new("schtasks")
        .args(["/query", "/tn", AUFGABE])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        // Eine vorhandene Aufgabe muss vor dem Austausch angehalten werden.
        let _ = Command::new("schtasks")
            .args(["/end", "/tn", AUFGABE])
            .output();
    }
    if Command::new("sc.exe")
        .args(["query", "NornsPruefbote"])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        let _ = Command::new("sc.exe")
            .args(["stop", "NornsPruefbote"])
            .output();
        ausfuehren("sc.exe", &["delete", "NornsPruefbote"])?;
    }
    if ich != ziel {
        let ende = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match std::fs::copy(&ich, &ziel) {
                Ok(_) => break,
                Err(_) if std::time::Instant::now() < ende => {
                    std::thread::sleep(std::time::Duration::from_millis(200))
                }
                Err(e) => {
                    return Err(format!(
                        "Die Programmdatei konnte nicht ersetzt werden: {e}"
                    ))
                }
            }
        }
    }
    std::fs::write(ort.join("quelle.txt"), quelle.to_string_lossy().as_bytes())
        .map_err(|e| e.to_string())?;
    let aufgabe = ort.join("aufgabe.xml");
    let text = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?><Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"><Triggers><BootTrigger><Enabled>true</Enabled></BootTrigger></Triggers><Principals><Principal id="System"><UserId>S-1-5-18</UserId><RunLevel>HighestAvailable</RunLevel></Principal></Principals><Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><StartWhenAvailable>true</StartWhenAvailable><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><RestartOnFailure><Interval>PT1M</Interval><Count>999</Count></RestartOnFailure></Settings><Actions Context="System"><Exec><Command>{}</Command><Arguments>--jetzt &quot;{}&quot;</Arguments></Exec></Actions></Task>"#,
        xml(&ziel.to_string_lossy()),
        xml(&quelle.to_string_lossy())
    );
    // schtasks liest Aufgaben-XML als UTF-16; Deklaration und Bytes muessen
    // uebereinstimmen, auch wenn der Datenbankpfad Umlaute enthaelt.
    let bytes: Vec<u8> = std::iter::once(0xfeff_u16)
        .chain(text.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    std::fs::write(&aufgabe, bytes).map_err(|e| e.to_string())?;
    ausfuehren(
        "schtasks",
        &[
            "/create",
            "/tn",
            AUFGABE,
            "/xml",
            aufgabe.to_str().ok_or("Der Aufgabenpfad ist unlesbar.")?,
            "/f",
        ],
    )?;
    // Idempotente benannte Regel. Der Port ist nur im lokalen Subnetz offen.
    let _ = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            "name=Norns XRF Agent",
        ])
        .output();
    ausfuehren(
        "netsh",
        &[
            "advfirewall",
            "firewall",
            "add",
            "rule",
            "name=Norns XRF Agent",
            "dir=in",
            "action=allow",
            "protocol=TCP",
            "localport=9614",
            "profile=any",
            "remoteip=localsubnet",
        ],
    )?;
    ausfuehren("schtasks", &["/run", "/tn", AUFGABE])?;
    // Nicht nur der Task-Aufruf: der neue Prozess muss authentisiert und mit
    // gueltiger Quelle antworten. Ein alter Dienst auf demselben Port reicht nicht.
    let ende = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if bereit(&wort) {
            break;
        }
        if std::time::Instant::now() >= ende {
            return Err("Die Aufgabe wurde angelegt, aber der neue Ergebnisdienst ist nicht bereit. Bitte die laufende alte Botenfassung und den Datenbankpfad pruefen.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    melden(&format!("Der Ergebnisdienst ist erreichbar und liest die Geraetedaten.\n\nVerbindungscode fuer Norns:\n{wort}\n\nDiesen Code einmal in Einstellungen → Pruefgeraet eintragen."),false);
    Ok(())
}

#[cfg(windows)]
fn bereit(wort: &str) -> bool {
    use std::io::{Read, Write};
    let Ok(mut s) = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:9614".parse().unwrap(),
        std::time::Duration::from_millis(300),
    ) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(1)));
    if write!(s, "GET /stand HTTP/1.0\r\nX-Norns-Bote: {wort}\r\n\r\n").is_err() {
        return false;
    }
    let mut text = String::new();
    if s.take(65536).read_to_string(&mut text).is_err() {
        return false;
    }
    text.split_once("\r\n\r\n")
        .and_then(|(_, b)| serde_json::from_str::<serde_json::Value>(b).ok())
        .is_some_and(|v| {
            v["protokoll"] == 2
                && v["quelleOk"] == true
                && v["fassung"] == env!("CARGO_PKG_VERSION")
        })
}

#[cfg(windows)]
pub fn entfernen() -> Result<(), String> {
    let _ = Command::new("schtasks")
        .args(["/end", "/tn", AUFGABE])
        .output();
    ausfuehren("schtasks", &["/delete", "/tn", AUFGABE, "/f"])?;
    ausfuehren(
        "netsh",
        &[
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            "name=Norns XRF Agent",
        ],
    )
}
#[cfg(windows)]
pub fn melden(text: &str, fehler: bool) {
    if std::env::var_os("NORNS_XRF_SILENT").is_some() {
        return;
    }
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(h: isize, t: *const u16, k: *const u16, a: u32) -> i32;
    }
    let t: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let k: Vec<u16> = "Norns XRF Agent".encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(0, t.as_ptr(), k.as_ptr(), if fehler { 0x30 } else { 0x40 });
    }
}
#[cfg(not(windows))]
pub fn installieren(_: Option<std::path::PathBuf>, _: bool) -> Result<(), String> {
    Err("Die Einrichtung ist fuer Windows. Zum Pruefen --jetzt verwenden.".into())
}
#[cfg(not(windows))]
pub fn entfernen() -> Result<(), String> {
    Err("Die Einrichtung ist fuer Windows.".into())
}
#[cfg(not(windows))]
pub fn melden(_: &str, _: bool) {}
