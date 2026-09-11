//! Dauerhafte Einrichtung, gemeinsam fuer Vordergrund und Windows-Aufgabe.
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub fn ort() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("NORNS_XRF_HOME") {
        return Ok(PathBuf::from(p));
    }
    #[cfg(windows)]
    {
        return std::env::var_os("ProgramData")
            .map(|p| PathBuf::from(p).join("Norns").join("XrfAgent"))
            .ok_or_else(|| "Windows nennt keine gemeinsame Programmablage.".into());
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .map(|p| PathBuf::from(p).join(".local/share/norns-xrf-agent"))
            .ok_or_else(|| "Die dauerhafte Programmablage fehlt.".into())
    }
}

pub fn zufall() -> Result<String, String> {
    let mut roh = [0u8; 32];
    getrandom::getrandom(&mut roh).map_err(|_| "Der Zufallsgeber ist nicht verfuegbar.")?;
    Ok(roh.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn wort(ort: &Path) -> Result<String, String> {
    std::fs::create_dir_all(ort)
        .map_err(|e| format!("Die Programmablage konnte nicht angelegt werden: {e}"))?;
    let pfad = ort.join("botenwort.txt");
    match std::fs::read_to_string(&pfad) {
        Ok(w) => return pruefen(w),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Der Verbindungscode konnte nicht gelesen werden: {e}"
            ))
        }
    }
    let w = zufall()?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(&pfad) {
        Ok(mut f) => {
            f.write_all(w.as_bytes())
                .and_then(|_| f.sync_all())
                .map_err(|e| format!("Der Verbindungscode konnte nicht gespeichert werden: {e}"))?;
            Ok(w)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => std::fs::read_to_string(pfad)
            .map_err(|e| e.to_string())
            .and_then(pruefen),
        Err(e) => Err(format!(
            "Der Verbindungscode konnte nicht angelegt werden: {e}"
        )),
    }
}
fn pruefen(w: String) -> Result<String, String> {
    let w = w.trim();
    if w.len() != 64 || !w.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(
            "Der gespeicherte Verbindungscode ist ungueltig. Er wurde nicht ersetzt.".into(),
        );
    }
    Ok(w.into())
}
