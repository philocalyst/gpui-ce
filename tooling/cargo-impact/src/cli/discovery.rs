//! Bounded discovery and translation into independent package/manifest experiments.

use super::Result;
use cargo_impact::{
    DownstreamSource, DownstreamSpec,
    config::{Config, DiscoveryConfig},
    discovery::{CratesIo, Discover, Discovery, GitHubSearch},
    http::Api,
};
use std::time::Instant;
use url::Url;

pub(super) fn discover(
    library: &str,
    config: &DiscoveryConfig,
    deadline: Option<Instant>,
) -> Result<Discovery> {
    let mut sources = Vec::new();
    if config.crates_io {
        match (CratesIo {
            api: bounded_api(Api::new(Url::parse("https://crates.io/")?, None)?, deadline),
            max_pages: config.max_pages,
        })
        .discover(library)
        {
            Ok(discovery) => sources.push(discovery),
            Err(error) => sources.push(Discovery {
                candidates: Vec::new(),
                notes: vec![format!("Discovery failed: crates.io: {error}")],
            }),
        }
    }
    if config.github {
        match (GitHubSearch {
            api: bounded_api(github_api()?, deadline),
            max_pages: config.max_pages,
        })
        .discover(library)
        {
            Ok(discovery) => sources.push(discovery),
            Err(error) => sources.push(Discovery {
                candidates: Vec::new(),
                notes: vec![format!("Discovery failed: GitHub: {error}")],
            }),
        }
    }
    Ok(Discovery::merge(sources))
}

fn bounded_api(api: Api, deadline: Option<Instant>) -> Api {
    match deadline {
        Some(deadline) => api.with_deadline(deadline),
        None => api,
    }
}

pub(super) fn candidates(discovery: &mut Discovery, config: &Config) -> Vec<DownstreamSpec> {
    let mut specs = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut omitted = 0;
    for candidate in discovery
        .candidates
        .iter()
        .take(config.discovery.max_repositories)
    {
        let name = format!(
            "{}/{}",
            candidate.repository.url().host_str().unwrap_or("forge"),
            candidate.repository.name()
        );
        // Published packages are independent experiments: one removed package must
        // not poison another valid workspace consumer in the same repository.
        let selections: Vec<(String, Option<String>)> = candidate
            .packages
            .iter()
            .map(|package| ("Cargo.toml".into(), Some(package.clone())))
            .chain(
                candidate
                    .manifests
                    .iter()
                    .map(|manifest| (manifest.clone(), None)),
            )
            .collect();
        let selections = if selections.is_empty() {
            vec![("Cargo.toml".into(), None)]
        } else {
            selections
        };
        for (manifest, package) in selections {
            let suffix = package.as_deref().unwrap_or(&manifest);
            let mut spec = DownstreamSpec {
                name: if suffix == "Cargo.toml" {
                    name.clone()
                } else {
                    format!("{name}:{suffix}")
                },
                source: DownstreamSource::Git {
                    url: candidate.repository.url().to_string(),
                    revision: "HEAD".into(),
                    forge: Some(candidate.repository.forge()),
                },
                manifest: manifest.into(),
                recipe: package.map(|package| {
                    let mut recipe = config.recipe.clone();
                    recipe.packages = vec![package];
                    recipe
                }),
            };
            if let Some(recipe) = config.overrides.get(&name) {
                spec.recipe = Some(recipe.clone());
            }
            config.apply_override(&mut spec);
            let identity = serde_json::to_string(&(
                &spec.source,
                &spec.manifest,
                spec.recipe.as_ref().unwrap_or(&config.recipe),
            ))
            .expect("selection is serializable");
            if seen.insert(identity) {
                if specs.len() < config.discovery.max_experiments {
                    specs.push(spec);
                } else {
                    omitted += 1;
                }
            }
        }
    }
    if omitted > 0 {
        discovery.notes.push(format!("Experiment budget: {} package/manifest selections omitted; {} indexed experiments selected.",omitted,specs.len()));
    }
    if discovery
        .candidates
        .iter()
        .any(|candidate| !candidate.packages.is_empty() && !candidate.manifests.is_empty())
    {
        discovery.notes.push("Registry package and code-search manifest selections can overlap after Cargo resolves the workspace; they are not evidence of distinct repositories or all users.".into());
    }
    if discovery.candidates.len() > config.discovery.max_repositories {
        discovery.notes.push(format!(
            "Build budget: {}/{} repositories selected.",
            config.discovery.max_repositories,
            discovery.candidates.len()
        ));
    }
    specs
}

pub(super) fn github_api() -> Result<Api> {
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .or_else(|| std::env::var("GH_TOKEN").ok());
    let base = std::env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com/".into());
    let base = Url::parse(&format!("{}/", base.trim_end_matches('/')))?;
    if base.scheme() != "https" || !base.username().is_empty() || base.password().is_some() {
        return Err("GitHub API requires an HTTPS URL without credentials".into());
    }
    Ok(Api::new(base, token)?)
}
