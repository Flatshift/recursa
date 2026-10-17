//! Where in the world a status report applies, and whether that includes the client.
//!
//! Operators' status pages say "degraded" for the whole service when one corner of it is,
//! and name that corner in a group label or in the incident's own words ("Service
//! degradation in Asia"). This reads those places so the Reliability page can call a
//! resolver operational for a client the report doesn't reach. A report that names no
//! place is treated as reaching everyone: a status is never cleared on a guess.

use serde_json::{Value, json};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Africa,
    Asia,
    Europe,
    LatinAmerica,
    MiddleEast,
    NorthAmerica,
    Oceania,
}

impl Region {
    pub fn name(self) -> &'static str {
        match self {
            Region::Africa => "Africa",
            Region::Asia => "Asia",
            Region::Europe => "Europe",
            Region::LatinAmerica => "Latin America & the Caribbean",
            Region::MiddleEast => "the Middle East",
            Region::NorthAmerica => "North America",
            Region::Oceania => "Oceania",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Region::Africa => "africa",
            Region::Asia => "asia",
            Region::Europe => "europe",
            Region::LatinAmerica => "latin-america",
            Region::MiddleEast => "middle-east",
            Region::NorthAmerica => "north-america",
            Region::Oceania => "oceania",
        }
    }

    fn from_key(k: &str) -> Option<Region> {
        use Region::*;
        [Africa, Asia, Europe, LatinAmerica, MiddleEast, NorthAmerica, Oceania].into_iter().find(|r| r.key() == k)
    }
}

use Region::*;

struct Country {
    code: &'static str,
    name: &'static str,
    regions: &'static [Region],
}

const fn c(code: &'static str, name: &'static str, regions: &'static [Region]) -> Country {
    Country { code, name, regions }
}

/// Every country and the regions a status page would file it under. Countries that span two
/// (Turkey, Russia, Egypt...) list both, so a report for either reaches them.
static COUNTRIES: &[Country] = &[
    c("AF", "Afghanistan", &[Asia]),
    c("AL", "Albania", &[Europe]),
    c("DZ", "Algeria", &[Africa]),
    c("AS", "American Samoa", &[Oceania]),
    c("AD", "Andorra", &[Europe]),
    c("AO", "Angola", &[Africa]),
    c("AI", "Anguilla", &[LatinAmerica]),
    c("AG", "Antigua and Barbuda", &[LatinAmerica]),
    c("AR", "Argentina", &[LatinAmerica]),
    c("AM", "Armenia", &[Europe, Asia, MiddleEast]),
    c("AW", "Aruba", &[LatinAmerica]),
    c("AU", "Australia", &[Oceania]),
    c("AT", "Austria", &[Europe]),
    c("AZ", "Azerbaijan", &[MiddleEast, Asia, Europe]),
    c("BS", "Bahamas", &[LatinAmerica]),
    c("BH", "Bahrain", &[MiddleEast]),
    c("BD", "Bangladesh", &[Asia]),
    c("BB", "Barbados", &[LatinAmerica]),
    c("BY", "Belarus", &[Europe]),
    c("BE", "Belgium", &[Europe]),
    c("BZ", "Belize", &[LatinAmerica]),
    c("BJ", "Benin", &[Africa]),
    c("BM", "Bermuda", &[NorthAmerica]),
    c("BT", "Bhutan", &[Asia]),
    c("BO", "Bolivia", &[LatinAmerica]),
    c("BA", "Bosnia and Herzegovina", &[Europe]),
    c("BW", "Botswana", &[Africa]),
    c("BR", "Brazil", &[LatinAmerica]),
    c("BN", "Brunei", &[Asia]),
    c("BG", "Bulgaria", &[Europe]),
    c("BF", "Burkina Faso", &[Africa]),
    c("BI", "Burundi", &[Africa]),
    c("KH", "Cambodia", &[Asia]),
    c("CM", "Cameroon", &[Africa]),
    c("CA", "Canada", &[NorthAmerica]),
    c("CV", "Cape Verde", &[Africa]),
    c("KY", "Cayman Islands", &[LatinAmerica]),
    c("CF", "Central African Republic", &[Africa]),
    c("TD", "Chad", &[Africa]),
    c("CL", "Chile", &[LatinAmerica]),
    c("CN", "China", &[Asia]),
    c("CO", "Colombia", &[LatinAmerica]),
    c("KM", "Comoros", &[Africa]),
    c("CG", "Republic of the Congo", &[Africa]),
    c("CD", "DR Congo", &[Africa]),
    c("CK", "Cook Islands", &[Oceania]),
    c("CR", "Costa Rica", &[LatinAmerica]),
    c("CI", "Ivory Coast", &[Africa]),
    c("HR", "Croatia", &[Europe]),
    c("CU", "Cuba", &[LatinAmerica]),
    c("CW", "Curacao", &[LatinAmerica]),
    c("CY", "Cyprus", &[Europe, MiddleEast]),
    c("CZ", "Czech Republic", &[Europe]),
    c("DK", "Denmark", &[Europe]),
    c("DJ", "Djibouti", &[Africa]),
    c("DM", "Dominica", &[LatinAmerica]),
    c("DO", "Dominican Republic", &[LatinAmerica]),
    c("EC", "Ecuador", &[LatinAmerica]),
    c("EG", "Egypt", &[Africa, MiddleEast]),
    c("SV", "El Salvador", &[LatinAmerica]),
    c("GQ", "Equatorial Guinea", &[Africa]),
    c("ER", "Eritrea", &[Africa]),
    c("EE", "Estonia", &[Europe]),
    c("SZ", "Eswatini", &[Africa]),
    c("ET", "Ethiopia", &[Africa]),
    c("FK", "Falkland Islands", &[LatinAmerica]),
    c("FO", "Faroe Islands", &[Europe]),
    c("FJ", "Fiji", &[Oceania]),
    c("FI", "Finland", &[Europe]),
    c("FR", "France", &[Europe]),
    c("GF", "French Guiana", &[LatinAmerica]),
    c("PF", "French Polynesia", &[Oceania]),
    c("GA", "Gabon", &[Africa]),
    c("GM", "Gambia", &[Africa]),
    c("DE", "Germany", &[Europe]),
    c("GH", "Ghana", &[Africa]),
    c("GI", "Gibraltar", &[Europe]),
    c("GR", "Greece", &[Europe]),
    c("GE", "Georgia (country)", &[Europe, Asia, MiddleEast]),
    c("GL", "Greenland", &[NorthAmerica]),
    c("GD", "Grenada", &[LatinAmerica]),
    c("GP", "Guadeloupe", &[LatinAmerica]),
    c("GU", "Guam", &[Oceania]),
    c("GT", "Guatemala", &[LatinAmerica]),
    c("GG", "Guernsey", &[Europe]),
    c("GN", "Guinea", &[Africa]),
    c("GW", "Guinea-Bissau", &[Africa]),
    c("GY", "Guyana", &[LatinAmerica]),
    c("HT", "Haiti", &[LatinAmerica]),
    c("HN", "Honduras", &[LatinAmerica]),
    c("HK", "Hong Kong", &[Asia]),
    c("HU", "Hungary", &[Europe]),
    c("IS", "Iceland", &[Europe]),
    c("IN", "India", &[Asia]),
    c("ID", "Indonesia", &[Asia]),
    c("IR", "Iran", &[MiddleEast]),
    c("IQ", "Iraq", &[MiddleEast]),
    c("IE", "Ireland", &[Europe]),
    c("IM", "Isle of Man", &[Europe]),
    c("IL", "Israel", &[MiddleEast]),
    c("IT", "Italy", &[Europe]),
    c("JM", "Jamaica", &[LatinAmerica]),
    c("JP", "Japan", &[Asia]),
    c("JO", "Jordan", &[MiddleEast]),
    c("KZ", "Kazakhstan", &[Asia, Europe]),
    c("KE", "Kenya", &[Africa]),
    c("KI", "Kiribati", &[Oceania]),
    c("XK", "Kosovo", &[Europe]),
    c("KW", "Kuwait", &[MiddleEast]),
    c("KG", "Kyrgyzstan", &[Asia]),
    c("LA", "Laos", &[Asia]),
    c("LV", "Latvia", &[Europe]),
    c("LB", "Lebanon", &[MiddleEast]),
    c("LS", "Lesotho", &[Africa]),
    c("LR", "Liberia", &[Africa]),
    c("LY", "Libya", &[Africa]),
    c("LI", "Liechtenstein", &[Europe]),
    c("LT", "Lithuania", &[Europe]),
    c("LU", "Luxembourg", &[Europe]),
    c("MO", "Macau", &[Asia]),
    c("MG", "Madagascar", &[Africa]),
    c("MW", "Malawi", &[Africa]),
    c("MY", "Malaysia", &[Asia]),
    c("MV", "Maldives", &[Asia]),
    c("ML", "Mali", &[Africa]),
    c("MT", "Malta", &[Europe]),
    c("MH", "Marshall Islands", &[Oceania]),
    c("MQ", "Martinique", &[LatinAmerica]),
    c("MR", "Mauritania", &[Africa]),
    c("MU", "Mauritius", &[Africa]),
    c("YT", "Mayotte", &[Africa]),
    c("MX", "Mexico", &[NorthAmerica, LatinAmerica]),
    c("FM", "Micronesia", &[Oceania]),
    c("MD", "Moldova", &[Europe]),
    c("MC", "Monaco", &[Europe]),
    c("MN", "Mongolia", &[Asia]),
    c("ME", "Montenegro", &[Europe]),
    c("MS", "Montserrat", &[LatinAmerica]),
    c("MA", "Morocco", &[Africa]),
    c("MZ", "Mozambique", &[Africa]),
    c("MM", "Myanmar", &[Asia]),
    c("NA", "Namibia", &[Africa]),
    c("NR", "Nauru", &[Oceania]),
    c("NP", "Nepal", &[Asia]),
    c("NL", "Netherlands", &[Europe]),
    c("NC", "New Caledonia", &[Oceania]),
    c("NZ", "New Zealand", &[Oceania]),
    c("NI", "Nicaragua", &[LatinAmerica]),
    c("NE", "Niger", &[Africa]),
    c("NG", "Nigeria", &[Africa]),
    c("NU", "Niue", &[Oceania]),
    c("KP", "North Korea", &[Asia]),
    c("MK", "North Macedonia", &[Europe]),
    c("MP", "Northern Mariana Islands", &[Oceania]),
    c("NO", "Norway", &[Europe]),
    c("OM", "Oman", &[MiddleEast]),
    c("PK", "Pakistan", &[Asia]),
    c("PW", "Palau", &[Oceania]),
    c("PS", "Palestine", &[MiddleEast]),
    c("PA", "Panama", &[LatinAmerica]),
    c("PG", "Papua New Guinea", &[Oceania]),
    c("PY", "Paraguay", &[LatinAmerica]),
    c("PE", "Peru", &[LatinAmerica]),
    c("PH", "Philippines", &[Asia]),
    c("PL", "Poland", &[Europe]),
    c("PT", "Portugal", &[Europe]),
    c("PR", "Puerto Rico", &[LatinAmerica]),
    c("QA", "Qatar", &[MiddleEast]),
    c("RE", "Reunion", &[Africa]),
    c("RO", "Romania", &[Europe]),
    c("RU", "Russia", &[Europe, Asia]),
    c("RW", "Rwanda", &[Africa]),
    c("KN", "Saint Kitts and Nevis", &[LatinAmerica]),
    c("LC", "Saint Lucia", &[LatinAmerica]),
    c("VC", "Saint Vincent and the Grenadines", &[LatinAmerica]),
    c("WS", "Samoa", &[Oceania]),
    c("SM", "San Marino", &[Europe]),
    c("ST", "Sao Tome and Principe", &[Africa]),
    c("SA", "Saudi Arabia", &[MiddleEast]),
    c("SN", "Senegal", &[Africa]),
    c("RS", "Serbia", &[Europe]),
    c("SC", "Seychelles", &[Africa]),
    c("SL", "Sierra Leone", &[Africa]),
    c("SG", "Singapore", &[Asia]),
    c("SK", "Slovakia", &[Europe]),
    c("SI", "Slovenia", &[Europe]),
    c("SB", "Solomon Islands", &[Oceania]),
    c("SO", "Somalia", &[Africa]),
    c("ZA", "South Africa", &[Africa]),
    c("KR", "South Korea", &[Asia]),
    c("SS", "South Sudan", &[Africa]),
    c("ES", "Spain", &[Europe]),
    c("LK", "Sri Lanka", &[Asia]),
    c("SD", "Sudan", &[Africa]),
    c("SR", "Suriname", &[LatinAmerica]),
    c("SE", "Sweden", &[Europe]),
    c("CH", "Switzerland", &[Europe]),
    c("SY", "Syria", &[MiddleEast]),
    c("TW", "Taiwan", &[Asia]),
    c("TJ", "Tajikistan", &[Asia]),
    c("TZ", "Tanzania", &[Africa]),
    c("TH", "Thailand", &[Asia]),
    c("TL", "Timor-Leste", &[Asia]),
    c("TG", "Togo", &[Africa]),
    c("TO", "Tonga", &[Oceania]),
    c("TT", "Trinidad and Tobago", &[LatinAmerica]),
    c("TN", "Tunisia", &[Africa]),
    c("TR", "Turkey", &[Europe, MiddleEast]),
    c("TM", "Turkmenistan", &[Asia]),
    c("TC", "Turks and Caicos Islands", &[LatinAmerica]),
    c("TV", "Tuvalu", &[Oceania]),
    c("UG", "Uganda", &[Africa]),
    c("UA", "Ukraine", &[Europe]),
    c("AE", "United Arab Emirates", &[MiddleEast]),
    c("GB", "United Kingdom", &[Europe]),
    c("US", "United States", &[NorthAmerica]),
    c("UY", "Uruguay", &[LatinAmerica]),
    c("UZ", "Uzbekistan", &[Asia]),
    c("VU", "Vanuatu", &[Oceania]),
    c("VA", "Vatican City", &[Europe]),
    c("VE", "Venezuela", &[LatinAmerica]),
    c("VN", "Vietnam", &[Asia]),
    c("VG", "British Virgin Islands", &[LatinAmerica]),
    c("VI", "US Virgin Islands", &[LatinAmerica]),
    c("YE", "Yemen", &[MiddleEast]),
    c("ZM", "Zambia", &[Africa]),
    c("ZW", "Zimbabwe", &[Africa]),
    c("AX", "Aland Islands", &[Europe]),
    c("BQ", "Bonaire", &[LatinAmerica]),
    c("BL", "Saint Barthelemy", &[LatinAmerica]),
    c("CC", "Cocos Islands", &[Oceania]),
    c("CX", "Christmas Island", &[Oceania]),
    c("EH", "Western Sahara", &[Africa]),
    c("JE", "Jersey", &[Europe]),
    c("MF", "Saint Martin", &[LatinAmerica]),
    c("NF", "Norfolk Island", &[Oceania]),
    c("PM", "Saint Pierre and Miquelon", &[NorthAmerica]),
    c("PN", "Pitcairn Islands", &[Oceania]),
    c("SH", "Saint Helena", &[Africa]),
    c("SX", "Sint Maarten", &[LatinAmerica]),
    c("TK", "Tokelau", &[Oceania]),
    c("WF", "Wallis and Futuna", &[Oceania]),
];

/// Other names a status page might use for a country, and the 50 US states: "New Mexico"
/// must read as the United States, not as Mexico.
const ALIASES: &[(&str, &str)] = &[
    ("usa", "US"),
    ("america", "US"),
    ("uk", "GB"),
    ("britain", "GB"),
    ("great britain", "GB"),
    ("england", "GB"),
    ("scotland", "GB"),
    ("wales", "GB"),
    ("northern ireland", "GB"),
    ("uae", "AE"),
    ("czechia", "CZ"),
    ("cote d ivoire", "CI"),
    ("korea", "KR"),
    ("republic of korea", "KR"),
    ("hongkong", "HK"),
    ("the netherlands", "NL"),
    ("holland", "NL"),
    ("burma", "MM"),
    ("macao", "MO"),
    ("congo", "CD"),
    ("democratic republic of the congo", "CD"),
    ("turkiye", "TR"),
    ("russian federation", "RU"),
    ("viet nam", "VN"),
    ("alabama", "US"),
    ("alaska", "US"),
    ("arizona", "US"),
    ("arkansas", "US"),
    ("california", "US"),
    ("colorado", "US"),
    ("connecticut", "US"),
    ("delaware", "US"),
    ("florida", "US"),
    ("georgia", "US"),
    ("hawaii", "US"),
    ("idaho", "US"),
    ("illinois", "US"),
    ("indiana", "US"),
    ("iowa", "US"),
    ("kansas", "US"),
    ("kentucky", "US"),
    ("louisiana", "US"),
    ("maine", "US"),
    ("maryland", "US"),
    ("massachusetts", "US"),
    ("michigan", "US"),
    ("minnesota", "US"),
    ("mississippi", "US"),
    ("missouri", "US"),
    ("montana", "US"),
    ("nebraska", "US"),
    ("nevada", "US"),
    ("new hampshire", "US"),
    ("new jersey", "US"),
    ("new mexico", "US"),
    ("new york", "US"),
    ("north carolina", "US"),
    ("north dakota", "US"),
    ("ohio", "US"),
    ("oklahoma", "US"),
    ("oregon", "US"),
    ("pennsylvania", "US"),
    ("rhode island", "US"),
    ("south carolina", "US"),
    ("south dakota", "US"),
    ("tennessee", "US"),
    ("texas", "US"),
    ("utah", "US"),
    ("vermont", "US"),
    ("virginia", "US"),
    ("washington", "US"),
    ("west virginia", "US"),
    ("wisconsin", "US"),
    ("wyoming", "US"),
    ("ontario", "CA"),
    ("quebec", "CA"),
    ("british columbia", "CA"),
    ("alberta", "CA"),
];

/// Regions by the names status pages use, including the broad ones (EMEA, the Americas).
const AREAS: &[(&str, &[Region])] = &[
    ("africa", &[Africa]),
    ("asia", &[Asia]),
    ("asia pacific", &[Asia, Oceania]),
    ("apac", &[Asia, Oceania]),
    ("europe", &[Europe]),
    ("emea", &[Europe, MiddleEast, Africa]),
    ("north america", &[NorthAmerica]),
    ("south america", &[LatinAmerica]),
    ("latin america", &[LatinAmerica]),
    ("latam", &[LatinAmerica]),
    ("central america", &[LatinAmerica]),
    ("caribbean", &[LatinAmerica]),
    ("the americas", &[NorthAmerica, LatinAmerica]),
    ("americas", &[NorthAmerica, LatinAmerica]),
    ("middle east", &[MiddleEast]),
    ("oceania", &[Oceania]),
    ("australasia", &[Oceania]),
    ("pacific", &[Oceania, Asia]),
];

/// Wording that says a report is not about one place.
const EVERYWHERE: &[&str] = &[
    "worldwide",
    "globally",
    "all regions",
    "all locations",
    "all data centers",
    "all data centres",
    "all pops",
    "every region",
    "multiple regions",
    "several regions",
];

#[derive(Clone, Copy)]
enum Target {
    Country(&'static str),
    Areas(&'static [Region]),
}

/// Every phrase worth looking for, longest first so "new mexico" is read before "mexico".
fn phrases() -> &'static [(String, Target)] {
    static P: OnceLock<Vec<(String, Target)>> = OnceLock::new();
    P.get_or_init(|| {
        let mut v: Vec<(String, Target)> = COUNTRIES.iter().map(|k| (norm(k.name), Target::Country(k.code))).collect();
        v.extend(ALIASES.iter().map(|(a, code)| (norm(a), Target::Country(code))));
        v.extend(AREAS.iter().map(|(a, rs)| (norm(a), Target::Areas(rs))));
        v.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        v
    })
}

/// Lowercase words separated by single spaces, padded with one at each end.
fn norm(s: &str) -> String {
    let mut out = String::from(" ");
    for ch in s.chars() {
        let ch = match ch {
            'é' | 'è' | 'ê' | 'É' => 'e',
            'ô' | 'ó' => 'o',
            'ã' | 'á' | 'à' | 'â' => 'a',
            'í' => 'i',
            'ü' | 'ú' => 'u',
            'ç' => 'c',
            c => c,
        };
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

fn country(code: &str) -> Option<&'static Country> {
    COUNTRIES.iter().find(|k| k.code == code)
}

/// The places a report names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Places {
    pub regions: Vec<Region>,
    pub countries: Vec<&'static str>,
    /// What the report called them, in the order it did; a country carries its code.
    names: Vec<(String, Option<&'static str>)>,
}

impl Places {
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty() && self.countries.is_empty()
    }

    /// "Asia", or "Singapore, Hong Kong, Taiwan and Japan". A country the report also covers
    /// by naming its region ("Asia ... Japan") isn't listed again.
    pub fn label(&self) -> String {
        let n: Vec<&str> = self
            .names
            .iter()
            .filter(|(_, code)| !code.and_then(country).is_some_and(|k| k.regions.iter().any(|r| self.regions.contains(r))))
            .map(|(name, _)| name.as_str())
            .collect();
        match n.len() {
            0 => String::new(),
            1 => n[0].to_string(),
            _ if n.len() <= 5 => format!("{} and {}", n[..n.len() - 1].join(", "), n[n.len() - 1]),
            _ => format!("{} and {} more", n[..4].join(", "), n.len() - 4),
        }
    }

    /// Whether the report reaches this client: it names the client's country or a region
    /// the client sits in. Asia takes in the Middle East, which is in Asia on a map.
    pub fn covers(&self, who: &Client) -> bool {
        self.countries.contains(&who.code)
            || self.regions.iter().any(|r| who.regions.contains(r) || (*r == Asia && who.regions.contains(&MiddleEast)))
    }

    pub fn merge(&mut self, other: Places) {
        for r in other.regions {
            if !self.regions.contains(&r) {
                self.regions.push(r);
            }
        }
        for k in other.countries {
            if !self.countries.contains(&k) {
                self.countries.push(k);
            }
        }
        for n in other.names {
            if !self.names.contains(&n) {
                self.names.push(n);
            }
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "label": self.label(),
            "regions": self.regions.iter().map(|r| r.key()).collect::<Vec<_>>(),
            "countries": self.countries,
        })
    }

    pub fn from_json(v: &Value) -> Places {
        let regions = v["regions"].as_array().into_iter().flatten().filter_map(|r| r.as_str().and_then(Region::from_key)).collect();
        let countries = v["countries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| COUNTRIES.iter().find(|k| Some(k.code) == c.as_str()).map(|k| k.code))
            .collect();
        Places {
            regions,
            countries,
            names: v["label"].as_str().filter(|l| !l.is_empty()).map(|l| vec![(l.to_string(), None)]).unwrap_or_default(),
        }
    }
}

/// Places named in a piece of text. Empty when it names none, or says the report is for
/// everywhere (then nothing narrows it).
pub fn detect(text: &str) -> Places {
    let text = capitals(&text.replace("U.S.", "United States").replace("U.K.", "United Kingdom"));
    let mut hay = norm(&text);
    if EVERYWHERE.iter().any(|e| hay.contains(&norm(e))) {
        return Places::default();
    }
    let mut found: Vec<(usize, Target, String)> = Vec::new();
    for (p, t) in phrases() {
        let mut from = 0;
        while let Some(i) = hay[from..].find(p.as_str()) {
            let at = from + i;
            found.push((at, *t, p.trim().to_string()));
            // Blank the match so a shorter phrase can't be found inside it.
            hay.replace_range(at + 1..at + p.len() - 1, &" ".repeat(p.len() - 2));
            from = at + p.len() - 1;
        }
    }
    found.sort_by_key(|f| f.0);
    let mut out = Places::default();
    for (_, t, said) in found {
        let mut one = Places::default();
        match t {
            Target::Country(code) => {
                if let Some(k) = country(code) {
                    one.countries.push(k.code);
                    one.names.push((k.name.to_string(), Some(k.code)));
                }
            }
            Target::Areas(rs) => {
                one.regions.extend(rs);
                one.names.push((titled(&said, rs), None));
            }
        }
        out.merge(one);
    }
    out
}

/// "US", "UK" and "EU" in capitals as words ("us" and "eu" in lowercase are not places).
fn capitals(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut word = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_alphanumeric() {
            word.push(ch);
            continue;
        }
        out.push_str(match word.as_str() {
            "US" | "USA" => "United States",
            "UK" => "United Kingdom",
            "EU" => "Europe",
            w => w,
        });
        word.clear();
        out.push(ch);
    }
    out.pop();
    out
}

/// A region as the page would write it ("asia pacific" → "Asia Pacific"); one region keeps its own name.
fn titled(said: &str, rs: &[Region]) -> String {
    if let [r] = rs
        && (r.name().to_lowercase().trim_start_matches("the ") == said || said == "latam")
    {
        return r.name().to_string();
    }
    said.split(' ')
        .map(|w| if w == "emea" || w == "apac" { w.to_uppercase() } else { w[..1].to_uppercase() + &w[1..] })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where the client is.
#[derive(Clone, Debug, PartialEq)]
pub struct Client {
    pub code: &'static str,
    pub name: &'static str,
    pub regions: &'static [Region],
}

impl Client {
    /// From an ISO 3166 country code, as geolocation gives it.
    pub fn from_code(code: &str) -> Option<Client> {
        let code = code.trim().to_ascii_uppercase();
        country(&code).map(|k| Client { code: k.code, name: k.name, regions: k.regions })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn us() -> Client {
        Client::from_code("us").unwrap()
    }

    #[test]
    fn a_region_in_a_title_names_that_region_only() {
        let p = detect("Service degradation in Asia");
        assert_eq!(p.regions, vec![Asia]);
        assert_eq!(p.label(), "Asia");
        assert!(!p.covers(&us()));
        assert!(p.covers(&Client::from_code("JP").unwrap()));
        // Asia on a map includes the Middle East.
        assert!(p.covers(&Client::from_code("AE").unwrap()));
    }

    #[test]
    fn quad9s_asia_notice_does_not_reach_the_united_states() {
        let p = detect(
            "Service degradation in Asia. The NTT GIN backbone is currently operating with reduced network capacity in Asia due to \
             multiple concurrent subsea cable outages. The most significant impact is to traffic transiting Singapore, Hong Kong and \
             Taiwan. On September 21, Typhoon Dujuan impacted Japan.",
        );
        assert!(p.regions.contains(&Asia));
        assert!(["SG", "HK", "TW", "JP"].iter().all(|c| p.countries.contains(c)), "{p:?}");
        // Named inside Asia, so the label doesn't list them again.
        assert_eq!(p.label(), "Asia");
        assert!(!p.covers(&us()));
        assert!(p.covers(&Client::from_code("SG").unwrap()));
    }

    #[test]
    fn countries_states_and_broad_areas() {
        assert!(detect("Resolvers in Frankfurt, Germany are slow").covers(&Client::from_code("DE").unwrap()));
        assert!(!detect("Resolvers in Frankfurt, Germany are slow").covers(&us()));
        // New Mexico is not Mexico, and a US state is the United States.
        assert_eq!(detect("Maintenance in New Mexico").countries, vec!["US"]);
        assert!(detect("Maintenance in Ashburn, Virginia").covers(&us()));
        assert!(detect("Impact across EMEA").covers(&Client::from_code("EG").unwrap()));
        assert!(!detect("Impact across EMEA").covers(&us()));
        assert!(detect("Impact in the Americas").covers(&us()));
        assert!(detect("Problems in the US and Canada").covers(&Client::from_code("CA").unwrap()));
        assert!(detect("Outage affecting U.S. customers").covers(&us()));
        // Mexico sits in North America on the status pages that list it there.
        assert!(detect("North America degraded").covers(&Client::from_code("MX").unwrap()));
    }

    #[test]
    fn lowercase_us_is_a_pronoun_and_names_nothing() {
        assert!(detect("Please contact us if this continues").is_empty());
        assert!(detect("an eu of nothing").is_empty());
        assert!(detect("US-based resolvers").covers(&us()));
    }

    #[test]
    fn a_report_for_everywhere_is_not_narrowed() {
        assert!(detect("Worldwide resolver outage; Asia hit hardest").is_empty());
        assert!(detect("Intermittent failures across all regions").is_empty());
        assert!(detect("Investigating elevated errors").is_empty());
    }

    #[test]
    fn every_country_code_is_unique_and_named() {
        let mut seen = std::collections::HashSet::new();
        for k in COUNTRIES {
            assert!(seen.insert(k.code), "duplicate {}", k.code);
            assert_eq!(k.code.len(), 2);
            assert!(!k.regions.is_empty(), "{}", k.name);
        }
        assert!(Client::from_code("zz").is_none());
        for (_, code) in ALIASES {
            assert!(country(code).is_some(), "alias for unknown {code}");
        }
    }
}
