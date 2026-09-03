//! A measurement, as the analyser stores it.
//!
//! The analyser keeps its measurements in an SQLite database, table
//! `summarys`. The result of each one is XML in the column `ResultContent`:
//!
//! ```xml
//! <Result><Layer Name="Bulk"><Group Name="Main">
//!   <Compo Name="Au" Fractal="83.52" /><Compo Name="Ag" Fractal="6.54" />
//! </Group></Layer></Result>
//! ```
//!
//! `Fractal` is the share in percent, as stated by the column `ReportUnit`.
//! The analyser always lists every element of its application, most of them
//! zero; the zeros are dropped here.

use serde_json::{json, Value};

/// One measurement.
#[derive(Debug, Clone)]
pub struct Messung {
    pub id: i64,
    /// The analyser application: `AuAgX`, `Pt`, `RubyTest` and so on.
    pub anwendung: String,
    pub probe: String,
    /// Unix time of the measurement.
    pub gemessen_am: i64,
    /// Element symbol and share in per mille (585.0 is 58.5 percent).
    pub elemente: Vec<(String, f64)>,
    /// Name of the analyser's per measurement file, used to find the spectrum.
    pub datei: String,
}

impl Messung {
    /// Gold content in per mille, if gold was found.
    pub fn gold_promille(&self) -> Option<f64> {
        self.elemente
            .iter()
            .find(|(symbol, _)| symbol == "Au")
            .map(|(_, anteil)| *anteil)
    }

    /// Karat, rounded to one decimal. 1000 per mille are 24 karat.
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

/// Reads the elements from the analyser's result XML.
///
/// Parsed by hand: the shape is fixed, it is documented above, and it comes
/// from the device rather than from the network.
pub fn elemente_lesen(xml: &str) -> Vec<(String, f64)> {
    let mut aus = Vec::new();
    for stueck in xml.split("<Compo").skip(1) {
        let ende = stueck.find('>').unwrap_or(stueck.len());
        let feld = &stueck[..ende];
        let (Some(symbol), Some(wert)) = (merkmal(feld, "Name"), merkmal(feld, "Fractal")) else {
            continue;
        };
        if !symbol_geformt(symbol) {
            continue;
        }
        let Ok(prozent) = wert.trim().parse::<f64>() else {
            continue;
        };
        // Zero means not found, not measured as zero.
        if prozent <= 0.0 {
            continue;
        }
        if aus.iter().any(|(s, _): &(String, f64)| s == symbol) {
            continue;
        }
        aus.push((symbol.to_string(), prozent * 10.0));
    }
    aus
}

fn merkmal<'a>(feld: &'a str, name: &str) -> Option<&'a str> {
    let suche = format!("{name}=\"");
    let start = feld.find(&suche)? + suche.len();
    let rest = &feld[start..];
    let ende = rest.find('"')?;
    Some(&rest[..ende])
}

/// One to three letters, first one capital. Gemstone applications report
/// traces such as Ga, Cs or Th, which are kept: the analyser distinguishes
/// natural from synthetic stones by exactly those traces.
fn symbol_geformt(symbol: &str) -> bool {
    let mut zeichen = symbol.chars();
    let Some(erstes) = zeichen.next() else {
        return false;
    };
    erstes.is_ascii_uppercase()
        && symbol.len() <= 3
        && symbol.chars().all(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A measurement as the analyser writes it.
    #[test]
    fn eine_echte_goldmessung() {
        let e = elemente_lesen(
            r#"<Result><Layer Name="Bulk"><Group Name="Main"><Compo Name="Au" Fractal="83.52" /><Compo Name="Ag" Fractal="6.54" /><Compo Name="Cu" Fractal="8.11" /><Compo Name="Zn" Fractal="0" /><Compo Name="Ni" Fractal="1.82" /></Group></Layer></Result>"#,
        );
        assert_eq!(e.len(), 4, "zeros are dropped: {e:?}");
        let m = Messung {
            id: 5,
            anwendung: "AuAgX".into(),
            probe: "39".into(),
            gemessen_am: 1_783_409_541,
            elemente: e,
            datei: "39(2026-07-07 15_32_21).xml".into(),
        };
        assert!((m.gold_promille().expect("Gold") - 835.2).abs() < 0.001);
        // 835 per mille is 20.04 karat; one decimal is what counts.
        assert_eq!(m.karat(), Some(20.0));
    }

    /// A stone: no alloy, only traces, and they are kept.
    #[test]
    fn die_spuren_eines_steins_bleiben() {
        let e = elemente_lesen(
            r#"<Compo Name="Fe" Fractal="0.036" /><Compo Name="Cr" Fractal="0.422" /><Compo Name="Au" Fractal="0" />"#,
        );
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e.iter().any(|(s, _)| s == "Cr"));
    }

    /// Nothing is guessed: what is not an element symbol is dropped.
    #[test]
    fn was_kein_element_ist_faellt_heraus() {
        let e = elemente_lesen(
            r#"<Compo Name="Bulk" Fractal="12" /><Compo Name="au" Fractal="5" /><Compo Name="Cu" Fractal="7.5" />"#,
        );
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].0, "Cu");
    }
}
