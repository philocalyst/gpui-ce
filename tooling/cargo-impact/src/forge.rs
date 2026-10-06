//! Repository identity and source links are independent of discovery and compilation.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Forge {
    GitHub,
    GitLab,
    Gitea,
    Generic,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Repository {
    url: Url,
    forge: Forge,
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("invalid repository URL: {0}")]
    Url(#[from] url::ParseError),
    #[error(
        "repository must be an HTTPS URL without credentials, query, fragment, or unsafe path components"
    )]
    Unsafe,
}

impl Repository {
    pub fn parse(input: &str, forge: Option<Forge>) -> Result<Self, RepositoryError> {
        let mut url = Url::parse(input)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(RepositoryError::Unsafe);
        }
        let path = url
            .path()
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .to_owned();
        let components: Vec<_> = path.trim_start_matches('/').split('/').collect();
        if components.len() < 2
            || components.iter().any(|v| {
                v.is_empty()
                    || !v
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                    || *v == "."
                    || *v == ".."
            })
        {
            return Err(RepositoryError::Unsafe);
        }
        url.set_path(&path);
        let forge = forge.unwrap_or_else(|| match url.host_str() {
            Some("github.com") => Forge::GitHub,
            Some("gitlab.com") => Forge::GitLab,
            Some("codeberg.org") => Forge::Gitea,
            _ => Forge::Generic,
        });
        Ok(Self { url, forge })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }
    pub fn forge(&self) -> Forge {
        self.forge
    }
    pub fn name(&self) -> &str {
        self.url.path().trim_start_matches('/')
    }

    /// Pin links to the tested commit, rather than a branch that may move later.
    pub fn source_link(&self, revision: &str, file: &str, line: u32) -> Option<Url> {
        if self.forge == Forge::Generic
            || !revision.chars().all(|c| c.is_ascii_hexdigit())
            || !(7..=64).contains(&revision.len())
            || line == 0
        {
            return None;
        }
        let mut link = self.url.clone();
        {
            let mut segments = link.path_segments_mut().ok()?;
            if self.forge == Forge::GitLab {
                segments.push("-");
            }
            segments.push("blob").push(revision);
            for component in file.split('/') {
                if component.is_empty() || component == "." || component == ".." {
                    return None;
                }
                segments.push(component);
            }
        }
        link.set_fragment(Some(&format!("L{line}")));
        Some(link)
    }
}

impl<'de> Deserialize<'de> for Repository {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            url: String,
            forge: Forge,
        }
        let raw = Raw::deserialize(deserializer)?;
        Repository::parse(&raw.url, Some(raw.forge)).map_err(serde::de::Error::custom)
    }
}
