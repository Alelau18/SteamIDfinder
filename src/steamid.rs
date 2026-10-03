//! Offline Steam ID parsing and conversion.
//!
//! Only individual user accounts in the public universe are supported, which is what every
//! profile link points at.

use std::fmt;

/// SteamID64 of account ID 0: universe 1 (public), type 1 (individual), instance 1.
pub const ID64_BASE: u64 = 76_561_197_960_265_728;

/// An individual Steam account, stored as its SteamID64.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SteamId(u64);

impl SteamId {
    /// Builds from a 32-bit account ID. Account ID 0 is not a real account.
    pub fn from_account_id(account_id: u32) -> Option<Self> {
        (account_id != 0).then(|| Self(ID64_BASE + u64::from(account_id)))
    }

    /// Builds from a SteamID64, rejecting anything outside the individual-account range.
    pub fn from_id64(id64: u64) -> Option<Self> {
        let account_id = id64.checked_sub(ID64_BASE)?;
        u32::try_from(account_id)
            .ok()
            .and_then(Self::from_account_id)
    }

    pub fn id64(self) -> u64 {
        self.0
    }

    pub fn account_id(self) -> u32 {
        // In range by construction.
        (self.0 - ID64_BASE) as u32
    }

    pub fn steam2(self) -> String {
        let account_id = self.account_id();
        format!("STEAM_0:{}:{}", account_id & 1, account_id >> 1)
    }

    pub fn steam3(self) -> String {
        format!("[U:1:{}]", self.account_id())
    }

    pub fn profile_url(self) -> String {
        format!("https://steamcommunity.com/profiles/{}", self.0)
    }
}

impl fmt::Display for SteamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which notation the user typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Id64,
    Steam2,
    Steam3,
    AccountId,
    ProfileUrl,
    CustomUrl,
    VanityName,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Id64 => "SteamID64",
            Format::Steam2 => "SteamID2",
            Format::Steam3 => "SteamID3",
            Format::AccountId => "Account ID",
            Format::ProfileUrl => "Profile URL",
            Format::CustomUrl => "Custom URL",
            Format::VanityName => "Custom URL name",
        }
    }
}

/// What a lookup has to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Id(SteamId),
    /// A custom URL name, validated to `[A-Za-z0-9_-]`.
    Vanity(String),
}

impl Target {
    /// Best link available without a network round-trip.
    pub fn offline_url(&self) -> String {
        match self {
            Target::Id(id) => id.profile_url(),
            Target::Vanity(name) => format!("https://steamcommunity.com/id/{name}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub format: Format,
    pub target: Target,
}

/// Splits pasted text into candidate IDs.
pub fn split_inputs(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter(|token| !token.is_empty())
}

/// Parses one token in any supported notation.
pub fn parse(token: &str) -> Result<Parsed, String> {
    let token = token
        .trim()
        .trim_matches(|c: char| matches!(c, '<' | '>' | '"' | '\'' | '(' | ')' | '`'));
    if token.is_empty() {
        return Err("empty input".into());
    }
    if token.to_ascii_lowercase().contains("steamcommunity.com") {
        return parse_url(token);
    }
    if let Some(id) = parse_steam2(token)? {
        return Ok(found(Format::Steam2, id));
    }
    if let Some(id) = parse_steam3(token)? {
        return Ok(found(Format::Steam3, id));
    }
    if token.bytes().all(|b| b.is_ascii_digit()) {
        return parse_number(token);
    }
    if is_vanity_name(token) {
        return Ok(Parsed {
            format: Format::VanityName,
            target: Target::Vanity(token.to_string()),
        });
    }
    Err(format!(
        "“{token}” isn't a Steam ID, profile URL or custom URL name"
    ))
}

fn found(format: Format, id: SteamId) -> Parsed {
    Parsed {
        format,
        target: Target::Id(id),
    }
}

fn parse_number(token: &str) -> Result<Parsed, String> {
    let invalid = || format!("{token} isn't a valid SteamID64 or account ID");
    let value: u64 = token.parse().map_err(|_| invalid())?;
    if let Some(id) = SteamId::from_id64(value) {
        return Ok(found(Format::Id64, id));
    }
    if token.len() < 17
        && let Some(id) = u32::try_from(value).ok().and_then(SteamId::from_account_id)
    {
        return Ok(found(Format::AccountId, id));
    }
    Err(invalid())
}

/// `STEAM_X:Y:Z`, where account ID = Z * 2 + Y.
fn parse_steam2(token: &str) -> Result<Option<SteamId>, String> {
    let Some(rest) = strip_prefix_ignore_case(token, "STEAM_") else {
        return Ok(None);
    };
    let invalid = || format!("{token} isn't a valid SteamID2");
    let mut parts = rest.split(':');
    let (Some(universe), Some(y), Some(z), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    if !matches!(universe, "0" | "1") || !matches!(y, "0" | "1") {
        return Err(invalid());
    }
    let z: u64 = z.parse().map_err(|_| invalid())?;
    let account_id = z
        .checked_mul(2)
        .and_then(|v| v.checked_add(u64::from(y == "1")))
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(invalid)?;
    SteamId::from_account_id(account_id)
        .map(Some)
        .ok_or_else(invalid)
}

/// `[U:1:N]`, brackets optional.
fn parse_steam3(token: &str) -> Result<Option<SteamId>, String> {
    let inner = token
        .strip_prefix('[')
        .and_then(|t| t.strip_suffix(']'))
        .unwrap_or(token);
    let mut parts = inner.split(':');
    let (Some(kind), Some(universe), Some(n), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Ok(None);
    };
    if kind.len() != 1 || !kind.chars().all(|c| c.is_ascii_alphabetic()) {
        return Ok(None);
    }
    if !kind.eq_ignore_ascii_case("U") {
        return Err(format!(
            "{token} isn't a user account (only [U:1:…] IDs have profiles)"
        ));
    }
    let invalid = || format!("{token} isn't a valid SteamID3");
    if universe != "1" {
        return Err(invalid());
    }
    let account_id: u32 = n.parse().map_err(|_| invalid())?;
    SteamId::from_account_id(account_id)
        .map(Some)
        .ok_or_else(invalid)
}

fn parse_url(token: &str) -> Result<Parsed, String> {
    let invalid = || format!("{token} isn't a Steam profile URL");
    let lower = token.to_ascii_lowercase();
    let start =
        lower.find("steamcommunity.com/").ok_or_else(invalid)? + "steamcommunity.com/".len();
    let path = &token[start..];
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let (Some(kind), Some(value)) = (segments.next(), segments.next()) else {
        return Err(invalid());
    };
    let value = percent_decode_brackets(value);
    match kind.to_ascii_lowercase().as_str() {
        "profiles" => {
            let id = match value.parse::<u64>() {
                Ok(n) => SteamId::from_id64(n),
                Err(_) => parse_steam3(&value).ok().flatten(),
            };
            id.map(|id| found(Format::ProfileUrl, id))
                .ok_or_else(invalid)
        }
        "id" if is_vanity_name(&value) => Ok(Parsed {
            format: Format::CustomUrl,
            target: Target::Vanity(value),
        }),
        _ => Err(invalid()),
    }
}

/// Steam custom URLs are 2–32 characters of letters, digits, `_` and `-`.
fn is_vanity_name(s: &str) -> bool {
    (2..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Undoes the escaping browsers apply to `[U:1:N]` in a URL.
fn percent_decode_brackets(s: &str) -> String {
    s.replace("%5B", "[")
        .replace("%5b", "[")
        .replace("%5D", "]")
        .replace("%5d", "]")
        .replace("%3A", ":")
        .replace("%3a", ":")
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    const GABEN: u64 = 76_561_197_960_287_930;

    fn id_of(input: &str) -> (Format, u64) {
        match parse(input) {
            Ok(Parsed {
                format,
                target: Target::Id(id),
            }) => (format, id.id64()),
            other => panic!("{input}: expected an ID, got {other:?}"),
        }
    }

    #[test]
    fn conversions_round_trip() {
        let id = SteamId::from_id64(GABEN).unwrap();
        assert_eq!(id.account_id(), 22202);
        assert_eq!(id.steam2(), "STEAM_0:0:11101");
        assert_eq!(id.steam3(), "[U:1:22202]");
        assert_eq!(
            id.profile_url(),
            "https://steamcommunity.com/profiles/76561197960287930"
        );
        let odd = SteamId::from_account_id(22203).unwrap();
        assert_eq!(odd.steam2(), "STEAM_0:1:11101");
    }

    #[test]
    fn every_format_resolves_to_the_same_account() {
        for (input, format) in [
            ("76561197960287930", Format::Id64),
            ("STEAM_0:0:11101", Format::Steam2),
            ("steam_1:0:11101", Format::Steam2),
            ("[U:1:22202]", Format::Steam3),
            ("U:1:22202", Format::Steam3),
            ("[u:1:22202]", Format::Steam3),
            ("22202", Format::AccountId),
            (
                "https://steamcommunity.com/profiles/76561197960287930",
                Format::ProfileUrl,
            ),
            (
                "steamcommunity.com/profiles/76561197960287930/",
                Format::ProfileUrl,
            ),
            (
                "http://www.steamcommunity.com/profiles/76561197960287930/games?tab=all",
                Format::ProfileUrl,
            ),
            (
                "https://steamcommunity.com/profiles/[U:1:22202]",
                Format::ProfileUrl,
            ),
            (
                "https://steamcommunity.com/profiles/%5BU%3A1%3A22202%5D",
                Format::ProfileUrl,
            ),
            (
                "<https://steamcommunity.com/profiles/76561197960287930>",
                Format::ProfileUrl,
            ),
        ] {
            assert_eq!(id_of(input), (format, GABEN), "{input}");
        }
    }

    #[test]
    fn vanity_names_and_custom_urls() {
        assert_eq!(
            parse("gabelogannewell").unwrap(),
            Parsed {
                format: Format::VanityName,
                target: Target::Vanity("gabelogannewell".into())
            }
        );
        assert_eq!(
            parse("https://steamcommunity.com/id/robinwalker/").unwrap(),
            Parsed {
                format: Format::CustomUrl,
                target: Target::Vanity("robinwalker".into())
            }
        );
        assert_eq!(
            Target::Vanity("robinwalker".into()).offline_url(),
            "https://steamcommunity.com/id/robinwalker"
        );
    }

    #[test]
    fn rejects_garbage() {
        for input in [
            "",
            "0",
            "76561197960265728", // account ID 0
            "76561202255233024", // past the 32-bit account range
            "123456789012345678901",
            "99999999999", // too big for an account ID, too short for an ID64
            "STEAM_0:2:5",
            "STEAM_0:0",
            "STEAM_0:0:0",
            "[G:1:4]",
            "[U:2:5]",
            "https://steamcommunity.com/groups/valve",
            "https://steamcommunity.com/profiles/notanumber",
            "a",
            "has space?",
            "名前",
        ] {
            assert!(parse(input).is_err(), "{input:?} should be rejected");
        }
    }

    #[test]
    fn splits_batches() {
        let tokens: Vec<_> =
            split_inputs(" 76561197960287930, STEAM_0:0:11101;[U:1:22202]\n\tgabe ").collect();
        assert_eq!(
            tokens,
            [
                "76561197960287930",
                "STEAM_0:0:11101",
                "[U:1:22202]",
                "gabe"
            ]
        );
    }

    #[test]
    fn id64_range_edges() {
        assert!(SteamId::from_id64(ID64_BASE + 1).is_some());
        assert!(SteamId::from_id64(ID64_BASE + u64::from(u32::MAX)).is_some());
        assert!(SteamId::from_id64(ID64_BASE + u64::from(u32::MAX) + 1).is_none());
        assert!(SteamId::from_id64(ID64_BASE).is_none());
        assert!(SteamId::from_account_id(0).is_none());
    }
}
