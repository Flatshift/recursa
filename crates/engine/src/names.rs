//! Domain names for the Cached test: checking one, checking a list, and reading a pasted or imported file.
//!
//! The Cached test asks each resolver for names it probably holds already, so the numbers are the network
//! round trip plus the resolver's front end. The default is a handful of very popular sites; a list of your
//! own most-used sites measures what *your* repeat lookups cost.

use std::net::IpAddr;

/// The most names one list may hold. Each round asks one name, so more names only spread the samples thinner.
pub(crate) const MAX_NAMES: usize = 50;

/// What the Cached test asks unless the list is changed.
pub(crate) fn defaults() -> Vec<String> {
    crate::bench::CACHED_NAMES.iter().map(|s| s.to_string()).collect()
}

/// One name as a lower-case host name without a trailing dot, or why it can't be used. A pasted URL gives
/// its host; an address, a name with one label, or one with characters DNS doesn't carry is refused.
pub(crate) fn normalize(input: &str) -> Result<String, String> {
    let mut t = input.trim().trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>').to_ascii_lowercase();
    if let Some((_, rest)) = t.split_once("://") {
        t = rest.to_string();
    }
    let t = t.split(['/', '?', '#']).next().unwrap_or("");
    let t = t.rsplit_once('@').map_or(t, |(_, h)| h);
    let t = match t.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => t,
    };
    let t = t.trim_end_matches('.');
    if t.is_empty() {
        return Err("empty".into());
    }
    if t.parse::<IpAddr>().is_ok() {
        return Err("that's an address, not a name".into());
    }
    if !t.is_ascii() {
        return Err("use the punycode form (xn--…) for names with accents or other scripts".into());
    }
    if t.len() > 253 {
        return Err("longer than 253 characters".into());
    }
    let labels: Vec<&str> = t.split('.').collect();
    if labels.len() < 2 {
        return Err("needs a dot, like example.com".into());
    }
    for l in &labels {
        if l.is_empty() || l.len() > 63 {
            return Err("each part between dots must be 1 to 63 characters".into());
        }
        if !l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
            || l.starts_with('-')
            || l.ends_with('-')
        {
            return Err("only letters, digits and hyphens".into());
        }
    }
    if labels.last().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit())) {
        return Err("the last part can't be all digits".into());
    }
    Ok(t.to_string())
}

/// A list as the Cached test will use it: every name normalized, repeats removed, in the order given.
pub fn validate(list: Vec<String>) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for n in &list {
        let n = normalize(n).map_err(|why| format!("“{}”: {why}.", n.trim()))?;
        if !out.contains(&n) {
            out.push(n);
        }
    }
    if out.is_empty() {
        return Err("Add at least one name for the Cached test.".into());
    }
    if out.len() > MAX_NAMES {
        return Err(format!("A list holds at most {MAX_NAMES} names."));
    }
    Ok(out)
}

/// A name that was read but not kept, and why.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct Skipped {
    pub text: String,
    pub why: String,
}

/// Header cells that mark the column of names in an exported table.
const NAME_HEADERS: [&str; 10] = ["domain", "domains", "host", "hostname", "name", "qname", "query", "site", "website", "url"];

/// Reads names out of pasted or imported text: one per line, or separated by commas or spaces. `#` starts a
/// comment; URLs give their host; hosts-file lines and top-site lists work (the address or rank in front is
/// ignored). A table, such as a CSV export with a domain column, gives that column only. Returns the valid
/// names in order without repeats, and what was skipped.
pub fn parse_list(text: &str) -> (Vec<String>, Vec<Skipped>) {
    let rows: Vec<&str> = text.lines().map(|l| l.split('#').next().unwrap_or("").trim()).filter(|l| !l.is_empty()).collect();
    let tokens: Vec<String> = match name_column(&rows) {
        Some((sep, col, header)) => rows.iter().skip(usize::from(header)).filter_map(|r| fields(r, sep).into_iter().nth(col)).collect(),
        None => rows
            .iter()
            .flat_map(|r| r.split(|c: char| c.is_whitespace() || c == ',' || c == ';'))
            .filter(|t| !t.is_empty())
            .map(String::from)
            .collect(),
    };
    let (mut names, mut skipped): (Vec<String>, Vec<Skipped>) = (Vec::new(), Vec::new());
    for tok in tokens.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        // An address (hosts files) or a rank (1,google.com) in front of the name isn't a name.
        if tok.parse::<IpAddr>().is_ok() || tok.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        match normalize(tok) {
            Ok(n) if !names.contains(&n) => names.push(n),
            Ok(_) => {}
            Err(why) => skipped.push(Skipped { text: tok.chars().take(80).collect(), why }),
        }
    }
    (names, skipped)
}

/// Where the names are when the text is a table: the separator, the column, and whether the first row is a
/// header. A table is two or more rows, the first of at least two cells. The column is the one headed like
/// "domain" (the rows below may differ in length), or else the only one that is mostly names, in a table whose
/// rows all split into the same number of cells. With no such column, or several
/// (a pasted list with two names a line), the text is read as a plain list.
fn name_column(rows: &[&str]) -> Option<(char, usize, bool)> {
    if rows.len() < 2 {
        return None;
    }
    for sep in [',', '\t', ';'] {
        let table: Vec<Vec<String>> = rows.iter().map(|r| fields(r, sep)).collect();
        let width = table[0].len();
        if width < 2 {
            continue;
        }
        // A named column is enough, however ragged the rows below it are (a note with a stray comma, a short row).
        if let Some(col) = table[0].iter().position(|c| NAME_HEADERS.contains(&c.trim().to_ascii_lowercase().as_str())) {
            return Some((sep, col, true));
        }
        if table.iter().any(|r| r.len() != width) {
            continue;
        }
        let mostly: Vec<usize> =
            (0..width).filter(|&i| table.iter().filter(|r| normalize(&r[i]).is_ok()).count() * 2 > table.len()).collect();
        return match mostly[..] {
            [col] => Some((sep, col, normalize(&table[0][col]).is_err())),
            _ => None,
        };
    }
    None
}

/// The cells of one CSV-style row. A cell in double quotes may hold the separator.
fn fields(row: &str, sep: char) -> Vec<String> {
    let (mut out, mut cell, mut quoted) = (Vec::new(), String::new(), false);
    for c in row.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c == sep && !quoted => out.push(std::mem::take(&mut cell)),
            c => cell.push(c),
        }
    }
    out.push(cell);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned_up_or_refused() {
        assert_eq!(normalize("  WWW.Example.COM. ").as_deref(), Ok("www.example.com"));
        assert_eq!(normalize("https://user@News.Example.org:8443/path?q=1#x").as_deref(), Ok("news.example.org"));
        assert_eq!(normalize("xn--bcher-kva.example").as_deref(), Ok("xn--bcher-kva.example"));
        assert_eq!(normalize("_dmarc.example.com").as_deref(), Ok("_dmarc.example.com"));
        for bad in
            ["", "localhost", "192.0.2.1", "2001:db8::1", "bücher.example", "a..b.com", "-a.example.com", "exa mple.com", "example.123"]
        {
            assert!(normalize(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(normalize(&format!("{}.com", "a".repeat(64))).is_err());
    }

    #[test]
    fn a_list_is_validated_as_a_whole() {
        assert_eq!(validate(vec!["A.com".into(), "a.com.".into(), "b.org".into()]).unwrap(), ["a.com", "b.org"]);
        assert!(validate(vec![]).is_err());
        let e = validate(vec!["ok.com".into(), "nope".into()]).unwrap_err();
        assert!(e.contains("nope") && e.contains("needs a dot"), "{e}");
        let many: Vec<String> = (0..=MAX_NAMES).map(|i| format!("site{i}.example.com")).collect();
        assert!(validate(many).unwrap_err().contains("at most"));
        assert!(validate(defaults()).is_ok());
    }

    #[test]
    fn pasted_and_imported_lists_are_read_leniently() {
        let text = "# my sites\nexample.com, News.Example.org\n0.0.0.0 ads.tracker.net\n1,popular.example\nhttps://shop.example.com/cart\n\nbad name here\nexample.com\n999.999.999.999\n";
        let (names, skipped) = parse_list(text);
        assert_eq!(
            names,
            ["example.com", "news.example.org", "ads.tracker.net", "popular.example", "shop.example.com", "bad", "name", "here"]
                .iter()
                .filter(|n| n.contains('.'))
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        // "bad", "name" and "here" have no dot; the all-digit "address" is neither a name nor valid.
        assert!(skipped.iter().any(|s| s.text == "bad" && s.why.contains("needs a dot")), "{skipped:?}");
        assert_eq!(parse_list("").0.len(), 0);
    }

    #[test]
    fn a_table_gives_its_column_of_names() {
        let names = |t: &str| {
            let (n, s) = parse_list(t);
            assert!(s.is_empty(), "nothing should be skipped from {t:?}: {s:?}");
            n
        };
        // A ranked list, like Tranco's.
        assert_eq!(names("1,google.com\n2,youtube.com\n"), ["google.com", "youtube.com"]);
        // A query log: the Domain column, not the client's host name.
        assert_eq!(
            names("Time,Type,Domain,Client\n1700000000,A,example.com,macbook.lan\n1700000001,AAAA,news.example.org,phone.lan\n"),
            ["example.com", "news.example.org"]
        );
        // Quoted cells that hold the separator, and a tab-separated export.
        assert_eq!(
            names("domain,categories\ngoogle.com,\"Search Engines, Technology\"\napple.com,Technology\n"),
            ["google.com", "apple.com"]
        );
        assert_eq!(names("site\tvisits\nexample.com\t12\nexample.org\t4\n"), ["example.com", "example.org"]);
        // An unrecognized header over the only column of names is passed over.
        assert_eq!(names("Site Address,Visits\nexample.com,12\nexample.org,4\n"), ["example.com", "example.org"]);
        // Two names a line is a list, not a table.
        assert_eq!(names("a.com, b.com\nc.com, d.com\n"), ["a.com", "b.com", "c.com", "d.com"]);
        // Rows of different lengths below a named column still give that column only.
        assert_eq!(
            names("domain,category,note\nads.example,Ads\ntrack.example,Tracking,seen on tv, twice\nnews.example.org,News,\n"),
            ["ads.example", "track.example", "news.example.org"]
        );
    }
}
