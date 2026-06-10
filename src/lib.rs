use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use reqwest::blocking::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const STEAM_ENDPOINT: &str = "https://api.steampowered.com/IStoreService/GetAppList/v1/";
pub const REQUIRED_FILES: &[&str] = &[
    "apps.all.json",
    "apps.games.json",
    "apps.dlc.json",
    "apps.software.json",
    "apps.videos.json",
    "apps.hardware.json",
    "appids.all.json",
    "manifest.json",
    ".nojekyll",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum CatalogType {
    Game,
    Dlc,
    Software,
    Video,
    Hardware,
}

impl CatalogType {
    pub const ALL: [CatalogType; 5] = [
        CatalogType::Game,
        CatalogType::Dlc,
        CatalogType::Software,
        CatalogType::Video,
        CatalogType::Hardware,
    ];

    pub const ALL_CATALOG_TYPES: [CatalogType; 3] =
        [CatalogType::Game, CatalogType::Software, CatalogType::Video];

    pub fn as_str(self) -> &'static str {
        match self {
            CatalogType::Game => "game",
            CatalogType::Dlc => "dlc",
            CatalogType::Software => "software",
            CatalogType::Video => "video",
            CatalogType::Hardware => "hardware",
        }
    }

    pub fn file_name(self) -> &'static str {
        match self {
            CatalogType::Game => "apps.games.json",
            CatalogType::Dlc => "apps.dlc.json",
            CatalogType::Software => "apps.software.json",
            CatalogType::Video => "apps.videos.json",
            CatalogType::Hardware => "apps.hardware.json",
        }
    }

    pub fn include_key(self) -> &'static str {
        match self {
            CatalogType::Game => "include_games",
            CatalogType::Dlc => "include_dlc",
            CatalogType::Software => "include_software",
            CatalogType::Video => "include_videos",
            CatalogType::Hardware => "include_hardware",
        }
    }

    pub fn priority(self) -> usize {
        match self {
            CatalogType::Game => 0,
            CatalogType::Software => 1,
            CatalogType::Video => 2,
            CatalogType::Dlc => 3,
            CatalogType::Hardware => 4,
        }
    }

    pub fn from_type_name(value: &str) -> Option<Self> {
        match value {
            "game" => Some(CatalogType::Game),
            "dlc" => Some(CatalogType::Dlc),
            "software" => Some(CatalogType::Software),
            "video" => Some(CatalogType::Video),
            "hardware" => Some(CatalogType::Hardware),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeCatalogEntry {
    pub appid: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub last_modified: u64,
    pub price_change_number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllCatalogEntry {
    pub appid: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub types: Vec<String>,
    pub last_modified: u64,
    pub price_change_number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeStats {
    pub fetched: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub restored: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileStats {
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    pub generated_at: String,
    pub source: String,
    pub endpoint: String,
    pub all_includes: Vec<String>,
    pub all_excludes: Vec<String>,
    pub files: BTreeMap<String, FileStats>,
    pub stats: BTreeMap<String, TypeStats>,
}

#[derive(Debug, Deserialize)]
struct ApiResponseEnvelope {
    response: ApiResponse,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    #[serde(default)]
    apps: Vec<ApiApp>,
    #[serde(default)]
    have_more_results: bool,
    last_appid: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ApiApp {
    appid: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    last_modified: u64,
    #[serde(default)]
    price_change_number: u64,
}

pub fn default_output_dir() -> PathBuf {
    env::var_os("OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("dist"))
}

pub fn steam_api_key() -> Result<String> {
    let key = env::var("STEAM_WEB_API_KEY")
        .ok()
        .or_else(|| read_env_file_value("STEAM_WEB_API_KEY"))
        .context("Environment variable STEAM_WEB_API_KEY is required")?;
    if key.trim().is_empty() {
        bail!("Environment variable STEAM_WEB_API_KEY must not be empty");
    }
    Ok(key)
}

fn read_env_file_value(name: &str) -> Option<String> {
    let content = fs::read_to_string(".env").ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, value) = line.split_once('=')?;
        if key.trim() == name {
            return Some(value.trim().trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

pub fn unix_timestamp_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX_EPOCH")?
        .as_secs())
}

pub fn build_manifest(
    generated_at: String,
    type_catalogs: &BTreeMap<CatalogType, Vec<TypeCatalogEntry>>,
    all_catalog: &[AllCatalogEntry],
    appids: &[u32],
    stats: BTreeMap<String, TypeStats>,
) -> Manifest {
    let mut files = BTreeMap::new();

    for catalog_type in CatalogType::ALL {
        let entries = type_catalogs
            .get(&catalog_type)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let removed_count = entries
            .iter()
            .filter(|entry| entry.removed == Some(true))
            .count();
        files.insert(
            catalog_type.file_name().to_string(),
            FileStats {
                count: entries.len(),
                active_count: Some(entries.len().saturating_sub(removed_count)),
                removed_count: Some(removed_count),
            },
        );
    }

    let all_removed_count = all_catalog
        .iter()
        .filter(|entry| entry.removed == Some(true))
        .count();
    files.insert(
        "apps.all.json".to_string(),
        FileStats {
            count: all_catalog.len(),
            active_count: Some(all_catalog.len().saturating_sub(all_removed_count)),
            removed_count: Some(all_removed_count),
        },
    );
    files.insert(
        "appids.all.json".to_string(),
        FileStats {
            count: appids.len(),
            active_count: None,
            removed_count: None,
        },
    );

    Manifest {
        version: 1,
        generated_at,
        source: "IStoreService/GetAppList/v1".to_string(),
        endpoint: STEAM_ENDPOINT.to_string(),
        all_includes: CatalogType::ALL_CATALOG_TYPES
            .iter()
            .map(|kind| kind.as_str().to_string())
            .collect(),
        all_excludes: vec!["dlc".to_string(), "hardware".to_string()],
        files,
        stats,
    }
}

pub fn initial_manifest() -> Manifest {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let type_catalogs = CatalogType::ALL
        .into_iter()
        .map(|kind| (kind, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    let stats = CatalogType::ALL
        .into_iter()
        .map(|kind| (kind.as_str().to_string(), TypeStats::default()))
        .collect::<BTreeMap<_, _>>();
    build_manifest(now, &type_catalogs, &[], &[], stats)
}

pub fn ensure_output_layout(output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output directory {}", output_dir.display()))?;

    for file_name in [
        "apps.all.json",
        "apps.games.json",
        "apps.dlc.json",
        "apps.software.json",
        "apps.videos.json",
        "apps.hardware.json",
    ] {
        let path = output_dir.join(file_name);
        if !path.exists() {
            write_json(&path, &Vec::<serde_json::Value>::new())?;
        }
    }

    let appids_path = output_dir.join("appids.all.json");
    if !appids_path.exists() {
        write_json(&appids_path, &Vec::<u32>::new())?;
    }

    let manifest_path = output_dir.join("manifest.json");
    if !manifest_path.exists() {
        write_json(&manifest_path, &initial_manifest())?;
    }

    let nojekyll_path = output_dir.join(".nojekyll");
    if !nojekyll_path.exists() {
        fs::write(&nojekyll_path, "")
            .with_context(|| format!("Failed to create {}", nojekyll_path.display()))?;
    }

    Ok(())
}

pub fn read_json_or_default<T>(path: &Path) -> Result<T>
where
    T: DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }

    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(T::default());
    }

    serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {}", path.display()))
}

pub fn write_json<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let data = serde_json::to_vec(value).context("Failed to serialize JSON")?;
    fs::write(path, data).with_context(|| format!("Failed to write {}", path.display()))
}

pub fn validate_sorted_unique<T, F>(items: &[T], mut appid_of: F, label: &str) -> Result<()>
where
    F: FnMut(&T) -> u32,
{
    let mut previous = None;
    for item in items {
        let current = appid_of(item);
        if let Some(last) = previous {
            if current <= last {
                bail!(
                    "{} is not strictly sorted by appid ascending or contains duplicates at appid {}",
                    label,
                    current
                );
            }
        }
        previous = Some(current);
    }
    Ok(())
}

fn fetch_page(
    client: &Client,
    api_key: &str,
    catalog_type: CatalogType,
    last_appid: Option<u32>,
) -> Result<ApiResponse> {
    let mut input = json!({
        "include_games": false,
        "include_dlc": false,
        "include_software": false,
        "include_videos": false,
        "include_hardware": false,
        "max_results": 50000
    });
    input[catalog_type.include_key()] = json!(true);
    if let Some(value) = last_appid {
        input["last_appid"] = json!(value);
    }

    let response = client
        .get(STEAM_ENDPOINT)
        .query(&[("key", api_key), ("input_json", &input.to_string())])
        .send()
        .with_context(|| format!("Steam API request failed for {}", catalog_type.as_str()))?
        .error_for_status()
        .with_context(|| format!("Steam API returned an error for {}", catalog_type.as_str()))?;

    let envelope: ApiResponseEnvelope = response.json().with_context(|| {
        format!(
            "Failed to decode Steam API response for {}",
            catalog_type.as_str()
        )
    })?;

    Ok(envelope.response)
}

pub fn fetch_type(
    client: &Client,
    api_key: &str,
    catalog_type: CatalogType,
) -> Result<(Vec<TypeCatalogEntry>, TypeStats)> {
    let mut app_map = BTreeMap::<u32, TypeCatalogEntry>::new();
    let mut next_last_appid = None;
    let mut previous_last_appid = 0_u32;

    loop {
        let page = fetch_page(client, api_key, catalog_type, next_last_appid)?;
        for app in &page.apps {
            app_map.insert(
                app.appid,
                TypeCatalogEntry {
                    appid: app.appid,
                    name: app.name.clone(),
                    kind: catalog_type.as_str().to_string(),
                    last_modified: app.last_modified,
                    price_change_number: app.price_change_number,
                    removed: None,
                    removed_at: None,
                },
            );
        }

        if !page.have_more_results {
            break;
        }

        let fallback_last_appid = page.apps.last().map(|app| app.appid);
        let last_appid = page.last_appid.or(fallback_last_appid).ok_or_else(|| {
            anyhow!(
                "Steam API pagination for {} ended without last_appid and without page items",
                catalog_type.as_str()
            )
        })?;

        if last_appid <= previous_last_appid {
            bail!(
                "Steam API pagination for {} is not progressing: next last_appid {} <= previous {}",
                catalog_type.as_str(),
                last_appid,
                previous_last_appid
            );
        }

        previous_last_appid = last_appid;
        next_last_appid = Some(last_appid);
        println!(
            "type={} last_appid={} fetched={}",
            catalog_type.as_str(),
            last_appid,
            app_map.len()
        );
    }

    let entries = app_map.into_values().collect::<Vec<_>>();
    Ok((
        entries.clone(),
        TypeStats {
            fetched: entries.len(),
            ..TypeStats::default()
        },
    ))
}

pub fn merge_type_catalog(
    old_entries: Vec<TypeCatalogEntry>,
    fresh_entries: Vec<TypeCatalogEntry>,
    catalog_type: CatalogType,
    removed_at_timestamp: u64,
    mut stats: TypeStats,
) -> Result<(Vec<TypeCatalogEntry>, TypeStats)> {
    let mut old_map = old_entries
        .into_iter()
        .map(|entry| (entry.appid, entry))
        .collect::<BTreeMap<_, _>>();
    let fresh_map = fresh_entries
        .into_iter()
        .map(|entry| (entry.appid, entry))
        .collect::<BTreeMap<_, _>>();

    let mut merged = Vec::with_capacity(old_map.len().max(fresh_map.len()));

    for (appid, mut fresh_entry) in fresh_map {
        fresh_entry.kind = catalog_type.as_str().to_string();
        match old_map.remove(&appid) {
            Some(old_entry) => {
                let was_removed = old_entry.removed == Some(true);
                if was_removed {
                    stats.restored += 1;
                }

                let changed = old_entry.name != fresh_entry.name
                    || old_entry.last_modified != fresh_entry.last_modified
                    || old_entry.price_change_number != fresh_entry.price_change_number
                    || old_entry.kind != fresh_entry.kind
                    || was_removed;

                if changed {
                    stats.updated += 1;
                }

                merged.push(TypeCatalogEntry {
                    removed: None,
                    removed_at: None,
                    ..fresh_entry
                });
            }
            None => {
                stats.added += 1;
                merged.push(fresh_entry);
            }
        }
    }

    for (_, mut old_entry) in old_map {
        old_entry.kind = catalog_type.as_str().to_string();
        if old_entry.removed != Some(true) {
            old_entry.removed = Some(true);
            if old_entry.removed_at.is_none() {
                old_entry.removed_at = Some(removed_at_timestamp);
            }
            stats.removed += 1;
        }
        merged.push(old_entry);
    }

    merged.sort_by_key(|entry| entry.appid);
    validate_sorted_unique(&merged, |entry| entry.appid, catalog_type.file_name())?;
    Ok((merged, stats))
}

pub fn build_all_catalog(
    type_catalogs: &BTreeMap<CatalogType, Vec<TypeCatalogEntry>>,
) -> Result<Vec<AllCatalogEntry>> {
    let mut grouped = BTreeMap::<u32, Vec<(CatalogType, TypeCatalogEntry)>>::new();

    for catalog_type in CatalogType::ALL_CATALOG_TYPES {
        if let Some(entries) = type_catalogs.get(&catalog_type) {
            for entry in entries {
                grouped
                    .entry(entry.appid)
                    .or_default()
                    .push((catalog_type, entry.clone()));
            }
        }
    }

    let mut result = Vec::with_capacity(grouped.len());
    for (appid, mut variants) in grouped {
        variants.sort_by_key(|(catalog_type, _)| catalog_type.priority());
        let primary = variants
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("Missing primary variant for appid {}", appid))?;

        let mut type_names = variants
            .iter()
            .map(|(catalog_type, _)| catalog_type.as_str().to_string())
            .collect::<Vec<_>>();
        type_names.sort_by_key(|name| {
            CatalogType::from_type_name(name)
                .map(CatalogType::priority)
                .unwrap_or(usize::MAX)
        });
        type_names.dedup();

        let all_removed = variants
            .iter()
            .all(|(_, entry)| entry.removed == Some(true));
        let removed_at = if all_removed {
            variants
                .iter()
                .filter_map(|(_, entry)| entry.removed_at)
                .min()
        } else {
            None
        };

        result.push(AllCatalogEntry {
            appid,
            name: primary.1.name,
            kind: primary.0.as_str().to_string(),
            types: type_names,
            last_modified: primary.1.last_modified,
            price_change_number: primary.1.price_change_number,
            removed: all_removed.then_some(true),
            removed_at,
        });
    }

    validate_sorted_unique(&result, |entry| entry.appid, "apps.all.json")?;
    Ok(result)
}

pub fn build_appids(all_catalog: &[AllCatalogEntry], previous_appids: &[u32]) -> Vec<u32> {
    let mut appids = all_catalog
        .iter()
        .map(|entry| entry.appid)
        .chain(previous_appids.iter().copied())
        .collect::<Vec<_>>();
    appids.sort_unstable();
    appids.dedup();
    appids
}

pub fn validate_output_dir(output_dir: &Path) -> Result<()> {
    for file_name in REQUIRED_FILES {
        let path = output_dir.join(file_name);
        if !path.exists() {
            bail!("Required output file is missing: {}", path.display());
        }
    }

    for catalog_type in CatalogType::ALL {
        let path = output_dir.join(catalog_type.file_name());
        let raw: serde_json::Value = read_json_or_default(&path)?;
        let items = raw
            .as_array()
            .ok_or_else(|| anyhow!("{} must be a JSON array", path.display()))?;
        for item in items {
            let object = item
                .as_object()
                .ok_or_else(|| anyhow!("{} must contain only objects", path.display()))?;
            if object.contains_key("types") {
                bail!("{} must not contain a `types` field", path.display());
            }
        }

        let entries: Vec<TypeCatalogEntry> = serde_json::from_value(raw)
            .with_context(|| format!("{} has invalid item structure", path.display()))?;
        validate_sorted_unique(&entries, |entry| entry.appid, catalog_type.file_name())?;
        for entry in entries {
            if entry.kind != catalog_type.as_str() {
                bail!(
                    "{} contains appid {} with type `{}` instead of `{}`",
                    path.display(),
                    entry.appid,
                    entry.kind,
                    catalog_type.as_str()
                );
            }
        }
    }

    let all_path = output_dir.join("apps.all.json");
    let raw_all: serde_json::Value = read_json_or_default(&all_path)?;
    let all_items = raw_all
        .as_array()
        .ok_or_else(|| anyhow!("{} must be a JSON array", all_path.display()))?;
    for item in all_items {
        let object = item
            .as_object()
            .ok_or_else(|| anyhow!("{} must contain only objects", all_path.display()))?;
        let types = object
            .get("types")
            .and_then(|value| value.as_array())
            .ok_or_else(|| {
                anyhow!(
                    "{} requires `types` arrays on every item",
                    all_path.display()
                )
            })?;
        if types.is_empty() {
            bail!(
                "{} contains an item with an empty `types` array",
                all_path.display()
            );
        }
    }

    let all_entries: Vec<AllCatalogEntry> = serde_json::from_value(raw_all)
        .with_context(|| format!("{} has invalid item structure", all_path.display()))?;
    validate_sorted_unique(&all_entries, |entry| entry.appid, "apps.all.json")?;
    for entry in &all_entries {
        if entry.kind == "dlc" || entry.kind == "hardware" {
            bail!(
                "apps.all.json must not contain type `{}` for appid {}",
                entry.kind,
                entry.appid
            );
        }
        if entry.types.is_empty() {
            bail!(
                "apps.all.json contains appid {} with empty types",
                entry.appid
            );
        }
    }

    let appids_path = output_dir.join("appids.all.json");
    let appids: Vec<u32> = read_json_or_default(&appids_path)?;
    validate_sorted_unique(&appids, |appid| *appid, "appids.all.json")?;
    let all_appids = all_entries
        .iter()
        .map(|entry| entry.appid)
        .collect::<BTreeSet<_>>();
    let appid_set = appids.iter().copied().collect::<BTreeSet<_>>();
    let missing = all_appids
        .difference(&appid_set)
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!(
            "appids.all.json is missing {} appid values from apps.all.json; first missing appid is {}",
            missing.len(),
            missing[0]
        );
    }

    let manifest_path = output_dir.join("manifest.json");
    let manifest_content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("Failed to read {}", manifest_path.display()))?;
    let _: Manifest = serde_json::from_str(&manifest_content)
        .with_context(|| format!("{} has invalid structure", manifest_path.display()))?;

    Ok(())
}

pub fn generate_catalog(output_dir: &Path) -> Result<()> {
    ensure_output_layout(output_dir)?;

    let api_key = steam_api_key()?;
    let client = Client::builder()
        .user_agent("steam-apps-catalog/0.1.0")
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("Failed to build HTTP client")?;

    let removed_at_timestamp = unix_timestamp_now()?;
    let mut type_catalogs = BTreeMap::<CatalogType, Vec<TypeCatalogEntry>>::new();
    let mut stats_by_type = BTreeMap::<String, TypeStats>::new();

    for catalog_type in CatalogType::ALL {
        let old_entries: Vec<TypeCatalogEntry> =
            read_json_or_default(&output_dir.join(catalog_type.file_name()))?;
        let (fresh_entries, fetch_stats) = fetch_type(&client, &api_key, catalog_type)?;
        let (merged_entries, merged_stats) = merge_type_catalog(
            old_entries,
            fresh_entries,
            catalog_type,
            removed_at_timestamp,
            fetch_stats,
        )?;
        println!(
            "type={} fetched={} added={} updated={} removed={} restored={}",
            catalog_type.as_str(),
            merged_stats.fetched,
            merged_stats.added,
            merged_stats.updated,
            merged_stats.removed,
            merged_stats.restored
        );
        write_json(&output_dir.join(catalog_type.file_name()), &merged_entries)?;
        type_catalogs.insert(catalog_type, merged_entries);
        stats_by_type.insert(catalog_type.as_str().to_string(), merged_stats);
    }

    let all_catalog = build_all_catalog(&type_catalogs)?;
    let existing_appids: Vec<u32> = read_json_or_default(&output_dir.join("appids.all.json"))?;
    let appids = build_appids(&all_catalog, &existing_appids);
    let generated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let manifest = build_manifest(
        generated_at,
        &type_catalogs,
        &all_catalog,
        &appids,
        stats_by_type,
    );

    write_json(&output_dir.join("apps.all.json"), &all_catalog)?;
    write_json(&output_dir.join("appids.all.json"), &appids)?;
    write_json(&output_dir.join("manifest.json"), &manifest)?;
    ensure_output_layout(output_dir)?;
    validate_output_dir(output_dir)?;

    Ok(())
}

pub fn write_bootstrap_files(output_dir: &Path) -> Result<()> {
    ensure_output_layout(output_dir)?;

    let empty_type_catalog = Vec::<TypeCatalogEntry>::new();
    for catalog_type in CatalogType::ALL {
        write_json(
            &output_dir.join(catalog_type.file_name()),
            &empty_type_catalog,
        )?;
    }

    let all_catalog = Vec::<AllCatalogEntry>::new();
    let appids = Vec::<u32>::new();
    let type_catalogs = CatalogType::ALL
        .into_iter()
        .map(|kind| (kind, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    let stats = CatalogType::ALL
        .into_iter()
        .map(|kind| (kind.as_str().to_string(), TypeStats::default()))
        .collect::<BTreeMap<_, _>>();
    let manifest = build_manifest(
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        &type_catalogs,
        &all_catalog,
        &appids,
        stats,
    );

    write_json(&output_dir.join("apps.all.json"), &all_catalog)?;
    write_json(&output_dir.join("appids.all.json"), &appids)?;
    write_json(&output_dir.join("manifest.json"), &manifest)?;
    fs::write(output_dir.join(".nojekyll"), "").context("Failed to write .nojekyll")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_marks_missing_items_removed() {
        let old_entries = vec![TypeCatalogEntry {
            appid: 10,
            name: "Old".to_string(),
            kind: "game".to_string(),
            last_modified: 1,
            price_change_number: 1,
            removed: None,
            removed_at: None,
        }];

        let (merged, stats) = merge_type_catalog(
            old_entries,
            Vec::new(),
            CatalogType::Game,
            123,
            TypeStats::default(),
        )
        .expect("merge should succeed");

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].removed, Some(true));
        assert_eq!(merged[0].removed_at, Some(123));
        assert_eq!(stats.removed, 1);
    }

    #[test]
    fn build_all_catalog_uses_priority_order() {
        let mut catalogs = BTreeMap::new();
        catalogs.insert(
            CatalogType::Game,
            vec![TypeCatalogEntry {
                appid: 10,
                name: "Counter-Strike".to_string(),
                kind: "game".to_string(),
                last_modified: 3,
                price_change_number: 4,
                removed: None,
                removed_at: None,
            }],
        );
        catalogs.insert(
            CatalogType::Software,
            vec![TypeCatalogEntry {
                appid: 10,
                name: "Counter-Strike Tools".to_string(),
                kind: "software".to_string(),
                last_modified: 1,
                price_change_number: 2,
                removed: None,
                removed_at: None,
            }],
        );

        let all_catalog = build_all_catalog(&catalogs).expect("catalog should build");
        assert_eq!(all_catalog[0].kind, "game");
        assert_eq!(all_catalog[0].types, vec!["game", "software"]);
        assert_eq!(all_catalog[0].name, "Counter-Strike");
    }
}
