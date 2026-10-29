//! City hints from router names, for the people who turn name decoding on.
//!
//! Carriers put a place in the name of a router: an airport code (`dca01.atlas.cogentco.com`), a CLLI place code
//! (`dllstx09.us.bb.gin.ntt.net`) or a city (`Atlanta2.Level3.net`). Each rule below knows one network's habit by
//! plain string matching, so a name from an unknown network gives nothing rather than a guess. A hint is only kept
//! when the round trip is long enough to reach that city, and the map words it "consistent with", never "in".

use crate::edge::{AIRPORTS, KM_PER_MS, great_circle_km};

/// Where a network puts the place in a router name.
#[derive(Clone, Copy)]
enum How {
    /// A three-letter airport code, alone as a label (`ae1.iad.ntwk.msn.net`).
    Code,
    /// An airport code followed by digits (`iad23s61`, `dca01`, `ord1`).
    CodeDigits,
    /// A CLLI place code starting a label (`attga`, `dllstx09`).
    Clli,
    /// A city spelled out, maybe with a number (`atlanta2`, `charlotte`).
    City,
}

/// Name suffix and how that network writes places.
const RULES: &[(&str, How)] = &[
    ("level3.net", How::City),
    ("lumen.tech", How::City),
    ("centurylink.net", How::Clli),
    ("qwest.net", How::Clli),
    ("ntt.net", How::Clli),
    ("cogentco.com", How::CodeDigits),
    ("zayo.com", How::CodeDigits),
    ("telia.net", How::Code),
    ("twelve99.net", How::Code),
    ("gtt.net", How::CodeDigits),
    ("att.net", How::Clli),
    ("sbcglobal.net", How::Clli),
    ("alter.net", How::CodeDigits),
    ("verizon.net", How::CodeDigits),
    ("comcast.net", How::City),
    ("charter.com", How::Clli),
    ("rr.com", How::Clli),
    ("cox.net", How::Clli),
    ("he.net", How::CodeDigits),
    ("sprintlink.net", How::Code),
    ("as6453.net", How::CodeDigits),
    ("cw.net", How::Code),
    ("1e100.net", How::CodeDigits),
    ("googleusercontent.com", How::CodeDigits),
    ("facebook.com", How::CodeDigits),
    ("fbcdn.net", How::CodeDigits),
    ("fastly.net", How::CodeDigits),
    ("cloudfront.net", How::CodeDigits),
    ("cloudflare.com", How::CodeDigits),
    ("cloudflare.net", How::CodeDigits),
    ("msn.net", How::Code),
    ("akamai.net", How::Code),
    ("akamaitechnologies.com", How::Code),
];

/// CLLI place codes (the first five or six letters of a router label) and the airport that stands for the metro.
const CLLI: &[(&str, &str)] = &[
    ("attga", "ATL"),
    ("atlnga", "ATL"),
    ("dllstx", "DFW"),
    ("dlstx", "DFW"),
    ("cgcil", "ORD"),
    ("chcgil", "ORD"),
    ("nycmny", "JFK"),
    ("nwrknj", "EWR"),
    ("wswdc", "DCA"),
    ("washdc", "DCA"),
    ("asbnva", "IAD"),
    ("sttlwa", "SEA"),
    ("lsanca", "LAX"),
    ("snjsca", "SJC"),
    ("sntcca", "SJC"),
    ("miamfl", "MIA"),
    ("dnvrco", "DEN"),
    ("hstntx", "IAH"),
    ("phnxaz", "PHX"),
    ("snfcca", "SFO"),
    ("chrlnc", "CLT"),
];

/// City names as they appear in labels, and the airport that stands for the city.
const CITIES: &[(&str, &str)] = &[
    ("ashburn", "IAD"),
    ("atlanta", "ATL"),
    ("chicago", "ORD"),
    ("dallas", "DFW"),
    ("newyork", "JFK"),
    ("losangeles", "LAX"),
    ("seattle", "SEA"),
    ("miami", "MIA"),
    ("denver", "DEN"),
    ("charlotte", "CLT"),
    ("houston", "IAH"),
    ("sanjose", "SJC"),
    ("phoenix", "PHX"),
    ("boston", "BOS"),
    ("washington", "DCA"),
    ("frankfurt", "FRA"),
    ("london", "LHR"),
];

fn airport(code: &str) -> Option<String> {
    AIRPORTS.get(code).map(|_| code.to_string())
}

fn code_in(label: &str, how: How) -> Option<String> {
    match how {
        How::Code => (label.len() == 3 && label.bytes().all(|b| b.is_ascii_alphabetic())).then(|| airport(&label.to_uppercase())).flatten(),
        How::CodeDigits => {
            let b = label.as_bytes();
            (b.len() >= 4 && b[..3].iter().all(u8::is_ascii_alphabetic) && b[3].is_ascii_digit())
                .then(|| airport(&label[..3].to_uppercase()))
                .flatten()
        }
        How::Clli => CLLI.iter().find(|(p, _)| label.starts_with(p)).and_then(|(_, c)| airport(c)),
        How::City => {
            let word = label.trim_end_matches(|c: char| c.is_ascii_digit());
            CITIES.iter().find(|(c, _)| *c == word).and_then(|(_, c)| airport(c))
        }
    }
}

/// The city a router's name points to, when the round trip allows it. `best_ms` must be at least the fibre floor from
/// `you` (latitude, longitude) to the city: a faster reply can't have gone that far, so the name is stale or wrong.
pub fn hint(name: &str, best_ms: Option<f64>, you: Option<(f64, f64)>) -> Option<String> {
    let name = name.trim_end_matches('.').to_lowercase();
    let how = RULES.iter().find(|(s, _)| name == *s || name.ends_with(&format!(".{s}"))).map(|(_, h)| *h)?;
    let code = name.split(['.', '-']).find_map(|l| code_in(l, how))?;
    let (city, _, lat, lon) = AIRPORTS.get(&code)?;
    let floor = great_circle_km(you?, (*lat, *lon)) / KM_PER_MS;
    (best_ms? >= floor).then(|| city.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHARLOTTE: Option<(f64, f64)> = Some((35.2, -80.8));

    #[test]
    fn a_name_gives_a_city_only_when_the_round_trip_reaches_it() {
        // Ashburn is about 530 km from Charlotte: a floor of about 5 ms.
        assert_eq!(hint("be2832.ccr41.iad01.atlas.cogentco.com", Some(9.0), CHARLOTTE).as_deref(), Some("Ashburn, VA"));
        assert_eq!(hint("be2832.ccr41.iad01.atlas.cogentco.com", Some(1.5), CHARLOTTE), None);
        assert_eq!(hint("be2832.ccr41.iad01.atlas.cogentco.com", None, CHARLOTTE), None);
        assert_eq!(hint("be2832.ccr41.iad01.atlas.cogentco.com", Some(9.0), None), None);
    }

    #[test]
    fn each_style_of_name_is_read_and_unknown_networks_give_nothing() {
        let ms = Some(40.0);
        assert_eq!(hint("ae-2-52.edge4.Atlanta2.Level3.net", ms, CHARLOTTE).as_deref(), Some("Atlanta, GA"));
        assert_eq!(hint("cr1.attga.ip.att.net", ms, CHARLOTTE).as_deref(), Some("Atlanta, GA"));
        assert_eq!(hint("ae1.iad.ntwk.msn.net", ms, CHARLOTTE).as_deref(), Some("Ashburn, VA"));
        assert_eq!(hint("iad23s61-in-f14.1e100.net", ms, CHARLOTTE).as_deref(), Some("Ashburn, VA"));
        assert_eq!(hint("host.example.org", ms, CHARLOTTE), None);
        // The airport-code rule isn't fooled by a label that only starts with letters.
        assert_eq!(hint("server.example.cogentco.com", ms, CHARLOTTE), None);
    }
}
