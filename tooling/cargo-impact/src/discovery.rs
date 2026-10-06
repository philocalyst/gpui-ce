//! Candidate indexes provide evidence, not a promise of exhaustive ecosystem coverage.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    forge::{Repository, RepositoryError},
    http::{Api, ApiError},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Candidate {
    pub repository: Repository,
    pub manifests: BTreeSet<String>,
    pub packages: BTreeSet<String>,
    pub evidence: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Discovery {
    pub candidates: Vec<Candidate>,
    /// Limitations, failed providers, and omitted records travel with every report.
    pub notes: Vec<String>,
}

impl Discovery {
    pub fn merge(sources: impl IntoIterator<Item = Discovery>) -> Self {
        let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
        let mut notes = Vec::new();
        for source in sources {
            notes.extend(source.notes);
            for candidate in source.candidates {
                candidates
                    .entry(candidate.repository.url().to_string())
                    .and_modify(|existing| {
                        existing.manifests.extend(candidate.manifests.clone());
                        existing.packages.extend(candidate.packages.clone());
                        existing.evidence.extend(candidate.evidence.clone());
                    })
                    .or_insert(candidate);
            }
        }
        Self {
            candidates: candidates.into_values().collect(),
            notes,
        }
    }
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error("invalid Cargo package name")]
    Package,
}

pub trait Discover {
    fn discover(&self, library: &str) -> Result<Discovery, DiscoveryError>;
}

fn validate_package(library: &str) -> Result<(), DiscoveryError> {
    if library.is_empty()
        || library.len() > 64
        || !library
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(DiscoveryError::Package);
    }
    Ok(())
}

/// Registry reverse dependencies work regardless of the repository's forge.
pub struct CratesIo {
    pub api: Api,
    pub max_pages: u32,
}

impl Discover for CratesIo {
    fn discover(&self, library: &str) -> Result<Discovery, DiscoveryError> {
        validate_package(library)?;
        let mut result = Discovery::default();
        result.notes.push("crates.io covers published direct dependents; unpublished and transitive consumers need additional indexes.".into());
        let mut observed = 0;
        for page in 1..=self.max_pages {
            let response = self.api.get::<ReverseDependencies>(
                &format!("api/v1/crates/{library}/reverse_dependencies"),
                &[("page", page.to_string()), ("per_page", "100".into())],
            );
            let response = match response {
                Ok(response) => response,
                Err(error) if page > 1 => {
                    result.notes.push(format!("Discovery failed: crates.io page {page}: {error}; earlier candidates retained."));
                    break;
                }
                Err(error) => return Err(error.into()),
            };
            observed += response.dependencies.len();
            for version in response.versions {
                if version.yanked {
                    continue;
                }
                let Some(repository) = version.repository else {
                    result.notes.push(format!(
                        "{} {} has no repository URL",
                        version.package, version.num
                    ));
                    continue;
                };
                match Repository::parse(&repository, None) {
                    Ok(repository) => result.candidates.push(Candidate {
                        repository,
                        manifests: BTreeSet::new(),
                        packages: BTreeSet::from([version.package.clone()]),
                        evidence: BTreeSet::from([format!(
                            "crates.io: {} {}",
                            version.package, version.num
                        )]),
                    }),
                    Err(error) => result.notes.push(format!("{}: {error}", version.package)),
                }
            }
            if observed >= response.meta.total || response.dependencies.is_empty() {
                break;
            }
            if page == self.max_pages {
                result.notes.push(format!(
                    "crates.io pagination capped: {observed}/{} dependency records inspected",
                    response.meta.total
                ));
            }
        }
        if self.max_pages == 0 {
            result
                .notes
                .push("crates.io disabled by a zero page budget".into());
        }
        Ok(Discovery::merge([result]))
    }
}

#[derive(Deserialize)]
struct ReverseDependencies {
    dependencies: Vec<serde_json::Value>,
    versions: Vec<RegistryVersion>,
    meta: RegistryMeta,
}
#[derive(Deserialize)]
struct RegistryMeta {
    total: usize,
}
#[derive(Deserialize)]
struct RegistryVersion {
    #[serde(rename = "crate")]
    package: String,
    num: String,
    repository: Option<String>,
    #[serde(default)]
    yanked: bool,
}

pub struct GitHubSearch {
    pub api: Api,
    pub max_pages: u32,
}

impl Discover for GitHubSearch {
    fn discover(&self, library: &str) -> Result<Discovery, DiscoveryError> {
        validate_package(library)?;
        let mut result = Discovery::default();
        result.notes.push("GitHub code search covers indexed default-branch manifests, caps at 1,000 hits, and returns candidates that still require Cargo graph verification.".into());
        for page in 1..=self.max_pages.min(10) {
            let response = self.api.get::<SearchResponse>(
                "search/code",
                &[
                    ("q", format!("{library} filename:Cargo.toml")),
                    ("page", page.to_string()),
                    ("per_page", "100".into()),
                ],
            );
            let response = match response {
                Ok(response) => response,
                Err(error) if page > 1 => {
                    result.notes.push(format!("Discovery failed: GitHub page {page}: {error}; earlier candidates retained."));
                    break;
                }
                Err(error) => return Err(error.into()),
            };
            let count = response.items.len();
            if response.incomplete_results {
                result
                    .notes
                    .push(format!("GitHub returned incomplete results on page {page}"));
            }
            for item in response.items {
                if !safe_manifest(&item.path) {
                    result
                        .notes
                        .push("GitHub returned an unsafe manifest path; omitted".into());
                    continue;
                }
                result.candidates.push(Candidate {
                    repository: Repository::parse(
                        &item.repository.html_url,
                        Some(crate::forge::Forge::GitHub),
                    )?,
                    manifests: BTreeSet::from([item.path]),
                    packages: BTreeSet::new(),
                    evidence: BTreeSet::from(["GitHub code search".into()]),
                });
            }
            if page as usize * 100 >= response.total_count || count == 0 {
                break;
            }
            if page == self.max_pages.min(10) {
                result.notes.push(format!(
                    "GitHub search pagination capped: at most {}/{} hits inspected",
                    page * 100,
                    response.total_count
                ));
            }
        }
        if self.max_pages == 0 {
            result
                .notes
                .push("GitHub search disabled by a zero page budget".into());
        }
        Ok(Discovery::merge([result]))
    }
}

fn safe_manifest(path: &str) -> bool {
    !path.starts_with('/')
        && !path.contains('\\')
        && path.ends_with("Cargo.toml")
        && path
            .split('/')
            .all(|v| !v.is_empty() && v != ".." && v != ".")
}

#[derive(Deserialize)]
struct SearchResponse {
    total_count: usize,
    incomplete_results: bool,
    items: Vec<SearchItem>,
}
#[derive(Deserialize)]
struct SearchItem {
    path: String,
    repository: SearchRepository,
}
#[derive(Deserialize)]
struct SearchRepository {
    html_url: String,
}
