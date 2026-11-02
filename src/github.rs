//! Releases from GitHub's public API. Unauthenticated calls allow 60 an hour,
//! plenty for a check every few hours; `GITHUB_TOKEN` raises that if set.

use crate::games::Game;
use serde::Deserialize;

pub const USER_AGENT: &str = concat!("sjk-launcher/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub html_url: String,
    /// The release notes, in Markdown.
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
    /// `sha256:<hex>`, which GitHub computes for every uploaded asset.
    #[serde(default)]
    pub digest: Option<String>,
}

impl Release {
    /// `2026-10-27` from `2026-10-27T00:23:56Z`.
    /// The day it came out, `dd/mm/yyyy`.
    pub fn date(&self) -> String {
        let at = self.published_at.as_deref().unwrap_or_default();
        match (at.get(..4), at.get(5..7), at.get(8..10)) {
            (Some(year), Some(month), Some(day)) => format!("{day}/{month}/{year}"),
            _ => at.to_owned(),
        }
    }

    /// The first changes the notes list (`* Change by @someone in <link>`), plain.
    pub fn highlights(&self, most: usize) -> Vec<String> {
        let body = self.body.as_deref().unwrap_or_default();
        body.lines()
            .filter_map(|line| line.trim().strip_prefix("* ").or_else(|| line.trim().strip_prefix("- ")))
            .map(|line| {
                let line = line.split(" by @").next().unwrap_or(line);
                let line = line.split(" in https://").next().unwrap_or(line);
                line.trim().trim_end_matches('.').to_owned()
            })
            .filter(|line| !line.is_empty() && !line.starts_with("**"))
            .take(most)
            .collect()
    }

    pub fn asset_for(&self, game: Game) -> Option<&Asset> {
        let wanted = game.asset_name(&self.tag_name)?;
        self.assets.iter().find(|asset| asset.name == wanted)
    }
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_connect(Some(std::time::Duration::from_secs(15)))
        .build()
        .into()
}

/// The game's releases for this computer, newest first, without drafts.
pub fn releases(agent: &ureq::Agent, game: Game, prereleases: bool) -> Result<Vec<Release>, String> {
    let url = format!("https://api.github.com/repos/{}/releases?per_page=30", game.repository());
    let mut request = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    let token = std::env::var("GITHUB_TOKEN").ok().filter(|token| !token.is_empty());
    if let Some(token) = &token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(|error| format!("GitHub: {error}"))?;
    let releases: Vec<Release> = response
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_json()
        .map_err(|error| format!("GitHub's answer: {error}"))?;
    Ok(releases
        .into_iter()
        .filter(|release| !release.draft && game.is_release_tag(&release.tag_name))
        .filter(|release| prereleases || !release.prerelease)
        .filter(|release| release.asset_for(game).is_some())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::GAMES;

    /// `cargo test -- --ignored`: both repositories answer with a build for this computer.
    #[test]
    #[ignore = "asks GitHub"]
    fn both_games_have_a_release_for_this_computer() {
        let agent = agent();
        for game in GAMES {
            let releases = releases(&agent, game, false).unwrap();
            let latest = releases.first().unwrap_or_else(|| panic!("{} has no release", game.name()));
            let asset = latest.asset_for(game).unwrap();
            println!("{}: {} {} ({} bytes, {:?})", game.name(), latest.tag_name, asset.name, asset.size, asset.digest);
            assert!(asset.size > 0);
        }
    }
}
