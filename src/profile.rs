//! Everything that talks to Steam. Uses the keyless community endpoints:
//! `/?xml=1` for the profile summary and `/ajaxaliases` for previous names.

use std::fmt;
use std::time::Duration;

use crate::steamid::{SteamId, Target};

const COMMUNITY: &str = "https://steamcommunity.com";
const USER_AGENT: &str = concat!(
    "SteamIDfinder/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/Alelau18/SteamIDfinder)"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnlineState {
    Online,
    InGame(Option<String>),
    Offline,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub id: SteamId,
    pub name: String,
    pub avatar_url: Option<String>,
    pub online: OnlineState,
    /// `public`, `friendsonly` or `private`, as Steam reports it.
    pub privacy: String,
    pub vac_banned: bool,
    /// `None` when Steam says "None".
    pub trade_ban: Option<String>,
    pub custom_url: Option<String>,
}

/// One entry from Steam's previous-names list. `when` is Valve's display string.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Alias {
    #[serde(rename = "newname")]
    pub name: String,
    #[serde(rename = "timechanged")]
    pub when: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    NotFound(String),
    RateLimited,
    Network(String),
    BadResponse(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::NotFound(msg) => write!(f, "{msg}"),
            FetchError::RateLimited => {
                write!(f, "Steam is rate-limiting requests; try again in a minute")
            }
            FetchError::Network(msg) => write!(f, "network error: {msg}"),
            FetchError::BadResponse(msg) => write!(f, "unexpected reply from Steam: {msg}"),
        }
    }
}

impl From<ureq::Error> for FetchError {
    fn from(err: ureq::Error) -> Self {
        match err {
            ureq::Error::StatusCode(429) => FetchError::RateLimited,
            ureq::Error::StatusCode(code) => FetchError::BadResponse(format!("HTTP {code}")),
            other => FetchError::Network(other.to_string()),
        }
    }
}

#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .user_agent(USER_AGENT)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Client {
    pub fn profile(&self, target: &Target) -> Result<Profile, FetchError> {
        let url = match target {
            Target::Id(id) => format!("{COMMUNITY}/profiles/{id}/?xml=1"),
            Target::Vanity(name) => format!("{COMMUNITY}/id/{name}/?xml=1"),
        };
        let xml = self.agent.get(&url).call()?.body_mut().read_to_string()?;
        parse_profile_xml(&xml)
    }

    pub fn aliases(&self, id: SteamId) -> Result<Vec<Alias>, FetchError> {
        let url = format!("{COMMUNITY}/profiles/{id}/ajaxaliases");
        let json = self.agent.get(&url).call()?.body_mut().read_to_string()?;
        parse_aliases(&json)
    }

    pub fn download(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        Ok(self
            .agent
            .get(url)
            .call()?
            .body_mut()
            .with_config()
            .limit(5 * 1024 * 1024)
            .read_to_vec()?)
    }
}

pub fn parse_profile_xml(xml: &str) -> Result<Profile, FetchError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| FetchError::BadResponse(format!("invalid XML ({e})")))?;
    let root = doc.root_element();
    let child_text = |parent: roxmltree::Node, tag: &str| -> Option<String> {
        parent
            .children()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };

    if let Some(error) = child_text(root, "error") {
        return Err(FetchError::NotFound(error));
    }
    if !root.has_tag_name("profile") {
        return Err(FetchError::BadResponse(format!(
            "<{}> instead of <profile>",
            root.tag_name().name()
        )));
    }

    let id = child_text(root, "steamID64")
        .and_then(|s| s.parse().ok())
        .and_then(SteamId::from_id64)
        .ok_or_else(|| FetchError::BadResponse("missing steamID64".into()))?;
    let game = root
        .children()
        .find(|n| n.has_tag_name("inGameInfo"))
        .and_then(|info| child_text(info, "gameName"));
    let online = match child_text(root, "onlineState").as_deref() {
        Some("online") => OnlineState::Online,
        Some("in-game") => OnlineState::InGame(game),
        Some("offline") | None => OnlineState::Offline,
        Some(other) => OnlineState::Other(other.to_string()),
    };

    Ok(Profile {
        id,
        name: child_text(root, "steamID").unwrap_or_default(),
        avatar_url: child_text(root, "avatarFull").filter(|u| u.starts_with("https://")),
        online,
        privacy: child_text(root, "privacyState").unwrap_or_else(|| "unknown".into()),
        vac_banned: child_text(root, "vacBanned").is_some_and(|v| v != "0"),
        trade_ban: child_text(root, "tradeBanState").filter(|s| s != "None"),
        custom_url: child_text(root, "customURL"),
    })
}

pub fn parse_aliases(json: &str) -> Result<Vec<Alias>, FetchError> {
    serde_json::from_str(json).map_err(|e| FetchError::BadResponse(format!("aliases: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><profile>
	<steamID64>76561197960287930</steamID64>
	<steamID><![CDATA[Rabscuttle]]></steamID>
	<onlineState>in-game</onlineState>
	<privacyState>public</privacyState>
	<avatarFull><![CDATA[https://avatars.fastly.steamstatic.com/c5d5_full.jpg]]></avatarFull>
	<vacBanned>1</vacBanned>
	<tradeBanState>Probation</tradeBanState>
	<customURL><![CDATA[gabelogannewell]]></customURL>
	<inGameInfo><gameName><![CDATA[Half-Life 3]]></gameName></inGameInfo>
</profile>"#;

    #[test]
    fn parses_profile() {
        let p = parse_profile_xml(PROFILE).unwrap();
        assert_eq!(p.id.id64(), 76_561_197_960_287_930);
        assert_eq!(p.name, "Rabscuttle");
        assert_eq!(p.online, OnlineState::InGame(Some("Half-Life 3".into())));
        assert_eq!(p.privacy, "public");
        assert!(p.vac_banned);
        assert_eq!(p.trade_ban.as_deref(), Some("Probation"));
        assert_eq!(p.custom_url.as_deref(), Some("gabelogannewell"));
        assert_eq!(
            p.avatar_url.as_deref(),
            Some("https://avatars.fastly.steamstatic.com/c5d5_full.jpg")
        );
    }

    #[test]
    fn not_found_is_reported() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><response><error><![CDATA[The specified profile could not be found.]]></error></response>"#;
        assert_eq!(
            parse_profile_xml(xml),
            Err(FetchError::NotFound(
                "The specified profile could not be found.".into()
            ))
        );
    }

    #[test]
    fn html_is_a_bad_response() {
        assert!(matches!(
            parse_profile_xml("<!DOCTYPE html><html><body>busy</body></html>"),
            Err(FetchError::BadResponse(_))
        ));
    }

    #[test]
    fn parses_aliases() {
        let json = r#"[{"newname":"Robin","timechanged":"7 May, 2019 @ 11:04pm"},{"newname":"Sekiro","timechanged":"7 May, 2019 @ 9:13pm"}]"#;
        let aliases = parse_aliases(json).unwrap();
        assert_eq!(aliases.len(), 2);
        assert_eq!(aliases[1].name, "Sekiro");
        assert_eq!(aliases[1].when, "7 May, 2019 @ 9:13pm");
        assert!(parse_aliases("[]").unwrap().is_empty());
    }
}
