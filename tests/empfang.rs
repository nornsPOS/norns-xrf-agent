//! Echtes Programm, echte SQLite/WAL und HTTP. Keine Herstellerdaten.
use rusqlite::Connection;
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Probe {
    ort: PathBuf,
    db: Connection,
    kind: Child,
    port: u16,
}
impl Probe {
    fn neu() -> Self {
        let mut zufall = [0u8; 8];
        getrandom::getrandom(&mut zufall).unwrap();
        let ort =
            std::env::temp_dir().join(format!("norns-empfang-{}", u64::from_ne_bytes(zufall)));
        std::fs::create_dir(&ort).unwrap();
        std::fs::write(ort.join("botenwort.txt"), "a".repeat(64)).unwrap();
        let db = Connection::open(ort.join("samplesummary.db")).unwrap();
        db.execute_batch("CREATE TABLE summarys (KeyId INTEGER PRIMARY KEY, AppName TEXT, SampleName TEXT, MeasureTime INTEGER, InfoSaveFile TEXT, ResultContent TEXT); PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;").unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let kind = Command::new(env!("CARGO_BIN_EXE_norns-xrf-agent"))
            .args(["--jetzt", ort.join("samplesummary.db").to_str().unwrap()])
            .env("NORNS_XRF_HOME", &ort)
            .env("NORNS_XRF_LISTEN", format!("127.0.0.1:{port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            ort,
            db,
            kind,
            port,
        }
    }
    fn einfuegen(&self, id: i64, gold: &str) {
        self.db.execute("INSERT OR REPLACE INTO summarys VALUES (?1,'AuAgX','Pruefstueck',1780000000,'',?2)",
            rusqlite::params![id, format!("<Result><Compo Name=\"Au\" Fractal=\"{gold}\" /></Result>")]).unwrap();
    }
    fn antwort(&self, wort: &str) -> Option<(u16, Value)> {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        write!(
            s,
            "GET /messungen?seit=-1 HTTP/1.0\r\nX-Norns-Bote: {wort}\r\n\r\n"
        )
        .ok()?;
        let mut text = String::new();
        s.read_to_string(&mut text).ok()?;
        let (kopf, rumpf) = text.split_once("\r\n\r\n")?;
        Some((
            kopf.split_whitespace().nth(1)?.parse().ok()?,
            serde_json::from_str(rumpf).ok()?,
        ))
    }
    fn warten(&self, pruefen: impl Fn(&(u16, Value)) -> bool) -> (u16, Value) {
        let ende = Instant::now() + Duration::from_secs(8);
        let mut letzte = None;
        while Instant::now() < ende {
            if let Some(a) = self.antwort(&"a".repeat(64)) {
                if pruefen(&a) {
                    return a;
                }
                letzte = Some(a);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("Antwort bleibt falsch: {letzte:?}");
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.kind.kill();
        let _ = self.kind.wait();
        if let Ok(ersatz) = Connection::open_in_memory() {
            let db = std::mem::replace(&mut self.db, ersatz);
            drop(db);
            let _ = std::fs::remove_dir_all(&self.ort);
        } /* Windows kann die noch offene Test-DB erst am Prozessende freigeben. */
    }
}

#[test]
fn neue_wal_werte_korrekturen_und_quellfehler_erreichen_http() {
    let p = Probe::neu();
    for id in 1..=501 {
        p.einfuegen(id, "58.5");
    }
    let a = p.warten(|(s, v)| {
        *s == 200
            && v["messungen"]
                .as_array()
                .and_then(|a| a.last())
                .is_some_and(|m| m["id"] == 501)
    });
    assert_eq!(a.1["protokoll"], 2);
    assert_eq!(a.1["quelleOk"], true);
    assert_eq!(a.1["messungen"].as_array().unwrap().len(), 200);
    p.einfuegen(502, "58.5");
    p.warten(|(_, v)| {
        v["messungen"]
            .as_array()
            .and_then(|a| a.last())
            .is_some_and(|m| m["id"] == 502)
    });
    p.einfuegen(502, "75");
    p.warten(|(_, v)| {
        v["messungen"]
            .as_array()
            .and_then(|a| a.last())
            .is_some_and(|m| m["goldPromille"] == 750.0)
    });
    p.db.execute_batch("ALTER TABLE summarys RENAME TO voruebergehend;")
        .unwrap();
    let a = p.warten(|(s, _)| *s == 503);
    assert!(a.1["messungen"].as_array().unwrap().is_empty());
    assert_eq!(a.1["quelleOk"], false);
    p.db.execute_batch("ALTER TABLE voruebergehend RENAME TO summarys; DELETE FROM summarys;")
        .unwrap();
    // Erfolgreiches Leeren muss einen vorherigen Fehler ebenfalls heilen.
    p.warten(|(s, v)| *s == 200 && v["messungen"].as_array().is_some_and(|a| a.is_empty()));
    p.einfuegen(1, "99.9");
    p.warten(|(s, v)| *s == 200 && v["messungen"][0]["id"] == 1);
    assert_eq!(p.antwort("falsch").unwrap().0, 401);
}

#[test]
fn unvollstaendige_neueste_messung_ist_keine_alte_erfolgsmeldung() {
    let p = Probe::neu();
    p.einfuegen(1, "58.5");
    p.warten(|(s, v)| *s == 200 && v["messungen"][0]["id"] == 1);
    p.db.execute(
        "INSERT INTO summarys VALUES (2,'AuAgX','neu',1780000001,'','')",
        [],
    )
    .unwrap();
    p.warten(|(s, _)| *s == 503);
    p.einfuegen(2, "75");
    p.warten(|(s, v)| *s == 200 && v["messungen"][1]["goldPromille"] == 750.0);
}
