//! Portable configuration, resolved relative to its own file rather than the caller's cwd.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    DownstreamSource, DownstreamSpec,
    model::ExecutionOptions,
    runner::{BuildRecipe, Runner},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub library: Option<String>,
    pub recipe: BuildRecipe,
    pub discovery: DiscoveryConfig,
    pub downstreams: Vec<DownstreamSpec>,
    pub overrides: BTreeMap<String, BuildRecipe>,
    pub execution: ExecutionOptions,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscoveryConfig {
    pub crates_io: bool,
    pub github: bool,
    pub max_pages: u32,
    pub max_repositories: usize,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            crates_io: true,
            github: false,
            max_pages: 5,
            max_repositories: 20,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut config: Self = toml::from_str(&fs::read_to_string(path)?)?;
        let root = path
            .canonicalize()?
            .parent()
            .ok_or("configuration has no parent directory")?
            .to_owned();
        resolve_recipe(&mut config.recipe, &root);
        for spec in &mut config.downstreams {
            if let DownstreamSource::Local { path } = &mut spec.source {
                *path = resolve(&root, path);
            }
            if let Some(recipe) = &mut spec.recipe {
                resolve_recipe(recipe, &root);
            }
        }
        for recipe in config.overrides.values_mut() {
            resolve_recipe(recipe, &root);
        }
        Ok(config)
    }

    pub fn apply_override(&self, downstream: &mut DownstreamSpec) {
        if let Some(recipe) = self.overrides.get(&downstream.name) {
            downstream.recipe = Some(recipe.clone());
        }
    }

    pub fn requires_local_execution(&self) -> bool {
        !self.recipe.runner.is_isolated()
            || self.overrides.values().any(|r| !r.runner.is_isolated())
            || self
                .downstreams
                .iter()
                .filter_map(|s| s.recipe.as_ref())
                .any(|r| !r.runner.is_isolated())
    }
}

fn resolve_recipe(recipe: &mut BuildRecipe, root: &Path) {
    if let Runner::Nix { file, .. } = &mut recipe.runner {
        *file = resolve(root, file);
    }
}
fn resolve(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        root.join(path)
    }
}
