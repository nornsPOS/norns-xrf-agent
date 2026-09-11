//! Was eine Messung ist, und wie sie aus der Datenbank des Geräts kommt.
//!
//! ── WOHER DAS WISSEN STAMMT (03.09.2026) ───────────────────────────────────
//!
//! Nicht aus einem Handbuch, sondern aus dem Gerät selbst: Basel hat es ans
//! Netz gehängt, und auf seinem Wartungsstick lag die Datenbank mit 77
//! echten Messungen. Die Tafel heisst `summarys`, und das Ergebnis steht
//! als XML in der Spalte `ResultContent`:
//!
//! ```xml
//! <Result><Layer Name="Bulk"><Group Name="Main">
//!   <Compo Name="Au" Fractal="83.52" /><Compo Name="Ag" Fractal="6.54" /> …
//! </Group></Layer></Result>
//! ```
//!
//! `Fractal` ist der Anteil in PROZENT (die Spalte `ReportUnit` sagt „%").
//! Das Gerät nennt immer alle Elemente seiner Anwendung, die meisten mit
//! null; die Nullen fallen hier heraus.

use serde_json::{json, Value};

/// Eine Messung, so wie die Kasse sie braucht.
#[derive(Debug, Clone, PartialEq)]
pub struct Messung {
    pub id: i64,
    /// Die Anwendung des Geräts: `AuAgX`, `Pt`, `RubyTest` …
    pub anwendung: String,
    pub probe: String,
    /// Unix-Zeit der Messung, wie das Gerät sie schreibt.
    pub gemessen_am: i64,
    /// Elementsymbol und Anteil in PROMILLE (585.0 heisst 58,5 Prozent).
    pub elemente: Vec<(String, f64)>,
    /// Der Name der Einzeldatei des Geräts, für das Auffinden des Spektrums.
    pub datei: String,
}

impl Messung {
    /// Der Goldanteil in Promille, wenn Gold gefunden wurde.
    pub fn gold_promille(&self) -> Option<f64> {
        self.elemente
            .iter()
            .find(|(symbol, _)| symbol == "Au")
            .map(|(_, anteil)| *anteil)
    }

    /// Das Karat, auf eine Stelle gerundet. 1000 Promille sind 24 Karat.
    ///
    /// ⚠️ Dieselbe Rechnung wie in der Kasse (`pruefgeraet_deuter.rs`). Sie
    /// steht hier ein zweites Mal, weil der Bote allein auf dem Gerät läuft
    /// und die Kasse nicht mitbringen kann; beide Seiten prüfen sie.
    pub fn karat(&self) -> Option<f64> {
        self.gold_promille()
            .map(|p| (p.min(1000.0) * 24.0 / 1000.0 * 10.0).round() / 10.0)
    }

    pub fn als_json(&self) -> Value {
        json!({
            "id": self.id,
            "anwendung": self.anwendung,
            "probe": self.probe,
            "gemessenAm": self.gemessen_am,
            "datei": self.datei,
            "elemente": self.elemente.iter()
                .map(|(s, p)| json!({ "symbol": s, "promille": p }))
                .collect::<Vec<_>>(),
            "goldPromille": self.gold_promille(),
            "karat": self.karat(),
        })
    }
}

/// Die Elemente aus dem Ergebnis-XML des Geräts lesen.
///
/// Von Hand und ohne XML-Bibliothek: die Form ist eine einzige, sie steht
/// oben, und sie kommt aus dem Gerät — nicht aus dem Netz. Ein Fremdling
/// könnte hier nichts einschleusen, was mehr wäre als eine Zahl.
pub fn elemente_lesen(xml: &str) -> Result<Vec<(String, f64)>, String> {
    if xml.len() > 2 * 1024 * 1024 {
        return Err("Das Messergebnis ist zu gross.".into());
    }
    let doc = roxmltree::Document::parse(xml)
        .map_err(|_| "Das Messergebnis ist kein vollstaendiges XML.")?;
    let mut aus = Vec::new();
    let mut summe = 0.0;
    for feld in doc.descendants().filter(|n| n.has_tag_name("Compo")) {
        let symbol = feld.attribute("Name").ok_or("Ein Elementsymbol fehlt.")?;
        // Nicht-elementare Herstellerfelder (etwa Bulk) sind keine Anteile.
        if !symbol_geformt(symbol) {
            continue;
        }
        let prozent: f64 = feld
            .attribute("Fractal")
            .ok_or("Ein Elementanteil fehlt.")?
            .trim()
            .parse()
            .map_err(|_| "Ein Elementanteil ist unlesbar.")?;
        if !prozent.is_finite() || !(0.0..=100.0).contains(&prozent) {
            return Err("Ein Elementanteil ist ungueltig.".into());
        }
        if aus.iter().any(|(s, _)| s == symbol) {
            return Err("Ein Element steht mehrfach im Ergebnis.".into());
        }
        if prozent == 0.0 {
            continue;
        }
        summe += prozent;
        aus.push((symbol.to_string(), prozent * 10.0));
    }
    if aus.is_empty() || aus.len() > 32 || summe > 100.0000001 {
        return Err("Die Elementanteile sind unvollstaendig oder widerspruechlich.".into());
    }
    Ok(aus)
}

/// Ein bis drei Buchstaben, erster gross. Bei Steinen nennt das Gerät Spuren
/// wie Ga, Cs, Th — die bleiben, denn an ihnen unterscheidet es einen
/// natürlichen von einem gemachten Stein.
fn symbol_geformt(symbol: &str) -> bool {
    let mut zeichen = symbol.chars();
    let Some(erstes) = zeichen.next() else {
        return false;
    };
    erstes.is_ascii_uppercase() && symbol.len() <= 3 && zeichen.all(|c| c.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die echte Messung #5 vom 07.07.2026 aus Basels Gerät.
    #[test]
    fn eine_echte_goldmessung() {
        let e = elemente_lesen(
            r#"<Result><Layer Name="Bulk"><Group Name="Main"><Compo Name="Au" Fractal="83.52" /><Compo Name="Ag" Fractal="6.54" /><Compo Name="Cu" Fractal="8.11" /><Compo Name="Zn" Fractal="0" /><Compo Name="Ni" Fractal="1.82" /></Group></Layer></Result>"#,
        ).unwrap();
        assert_eq!(e.len(), 4, "die Nullen des Geräts fallen heraus: {e:?}");
        let m = Messung {
            id: 5,
            anwendung: "AuAgX".into(),
            probe: "39".into(),
            gemessen_am: 1_783_409_541,
            elemente: e,
            datei: "39(2026-07-07 15_32_21).xml".into(),
        };
        assert!((m.gold_promille().expect("Gold") - 835.2).abs() < 0.001);
        // 835 Promille sind rechnerisch 20,04 Karat; am Tresen zählt „20".
        assert_eq!(m.karat(), Some(20.0));
    }

    /// Ein Stein: keine Legierung, nur Spuren — und sie bleiben stehen.
    #[test]
    fn die_spuren_eines_steins_bleiben() {
        let e = elemente_lesen(
            r#"<Result><Compo Name="Fe" Fractal="0.036" /><Compo Name="Cr" Fractal="0.422" /><Compo Name="Au" Fractal="0" /></Result>"#,
        ).unwrap();
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e.iter().any(|(s, _)| s == "Cr"));
    }

    /// ⛔ Es wird nicht geraten: was kein Elementsymbol ist, fliegt heraus.
    #[test]
    fn was_kein_element_ist_faellt_heraus() {
        let e = elemente_lesen(
            r#"<Result><Compo Name="Bulk" Fractal="12" /><Compo Name="au" Fractal="5" /><Compo Name="Cu" Fractal="7.5" /></Result>"#,
        ).unwrap();
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].0, "Cu");
    }
}
