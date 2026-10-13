//! The online libraries of the shelf: ambientCG and Poly Haven, both CC0. Listing,
//! previews and downloads of PBR materials, which arrive as [`MaterialFiles`].

use crate::material::{self, MaterialFiles};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

/// An online library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    AmbientCg,
    PolyHaven,
}

impl Source {
    pub const ALL: [Self; 2] = [Self::AmbientCg, Self::PolyHaven];

    pub fn label(self) -> &'static str {
        match self {
            Self::AmbientCg => "ambientCG",
            Self::PolyHaven => "Poly Haven",
        }
    }

    pub fn url(self) -> &'static str {
        match self {
            Self::AmbientCg => "https://ambientcg.com",
            Self::PolyHaven => "https://polyhaven.com",
        }
    }
}

/// A material of an online library.
#[derive(Clone, Debug, PartialEq)]
pub struct OnlineAsset {
    pub source: Source,
    pub id: String,
    pub name: String,
    pub category: String,
    pub preview_url: String,
}

/// Which size of the maps to download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    K1,
    K2,
    K4,
}

impl Resolution {
    pub const ALL: [Self; 3] = [Self::K1, Self::K2, Self::K4];

    pub fn label(self) -> &'static str {
        match self {
            Self::K1 => "1K",
            Self::K2 => "2K",
            Self::K4 => "4K",
        }
    }
}

const USER_AGENT: &str = concat!("inkpainter3d/", env!("CARGO_PKG_VERSION"));

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(Duration::from_secs(300)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .build()
        .into()
}

/// Fetches a URL.
pub fn fetch(url: &str) -> Result<Vec<u8>> {
    let mut response = agent()
        .get(url)
        .call()
        .with_context(|| format!("Failed to download {url}"))?;
    response
        .body_mut()
        .with_config()
        .limit(1 << 31)
        .read_to_vec()
        .with_context(|| format!("Failed to read {url}"))
}

fn fetch_json(url: &str) -> Result<Value> {
    let bytes = fetch(url)?;
    serde_json::from_slice(&bytes).with_context(|| format!("{url} did not answer with JSON"))
}

/// A page of materials of a library matching `query` (empty: the most popular).
pub fn search(
    source: Source,
    query: &str,
    offset: usize,
    limit: usize,
) -> Result<Vec<OnlineAsset>> {
    match source {
        Source::AmbientCg => search_ambientcg(query, offset, limit),
        Source::PolyHaven => search_polyhaven(query, offset, limit),
    }
}

fn search_ambientcg(query: &str, offset: usize, limit: usize) -> Result<Vec<OnlineAsset>> {
    let mut url = format!(
        "https://ambientcg.com/api/v2/full_json?type=Material&limit={limit}&offset={offset}&include=previewData"
    );
    if query.trim().is_empty() {
        url.push_str("&sort=Popular");
    } else {
        // With a query the site orders by relevance.
        url.push_str("&q=");
        url.push_str(&encode(query.trim()));
    }
    let json = fetch_json(&url)?;
    let found = json["foundAssets"].as_array().cloned().unwrap_or_default();
    Ok(found
        .iter()
        .filter_map(|a| {
            let id = a["assetId"].as_str()?.to_owned();
            Some(OnlineAsset {
                source: Source::AmbientCg,
                name: a["displayName"].as_str().unwrap_or(&id).to_owned(),
                category: a["displayCategory"].as_str().unwrap_or_default().to_owned(),
                preview_url: a["previewImage"]["256-PNG"]
                    .as_str()
                    .or_else(|| a["previewImage"]["128-PNG"].as_str())?
                    .to_owned(),
                id,
            })
        })
        .collect())
}

fn search_polyhaven(query: &str, offset: usize, limit: usize) -> Result<Vec<OnlineAsset>> {
    // Poly Haven answers with every texture at once; the filtering is ours.
    let json = fetch_json("https://api.polyhaven.com/assets?t=textures")?;
    let Some(all) = json.as_object() else {
        bail!("Poly Haven answered with something unexpected");
    };
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let mut matches: Vec<(&String, &Value)> = all
        .iter()
        .filter(|(id, a)| {
            let haystack = format!(
                "{} {} {} {}",
                id,
                a["name"].as_str().unwrap_or_default(),
                a["tags"],
                a["categories"]
            )
            .to_lowercase();
            words.iter().all(|w| haystack.contains(w))
        })
        .collect();
    matches.sort_by_key(|(_, a)| std::cmp::Reverse(a["download_count"].as_u64().unwrap_or(0)));
    Ok(matches
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(id, a)| OnlineAsset {
            source: Source::PolyHaven,
            name: a["name"].as_str().unwrap_or(id).to_owned(),
            category: a["categories"]
                .as_array()
                .and_then(|c| c.first())
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            preview_url: format!(
                "https://cdn.polyhaven.com/asset_img/thumbs/{id}.png?width=256&height=256"
            ),
            id: id.clone(),
        })
        .collect())
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Downloads a material's maps.
pub fn download(asset: &OnlineAsset, resolution: Resolution) -> Result<MaterialFiles> {
    match asset.source {
        Source::AmbientCg => download_ambientcg(asset, resolution),
        Source::PolyHaven => download_polyhaven(asset, resolution),
    }
}

fn download_ambientcg(asset: &OnlineAsset, resolution: Resolution) -> Result<MaterialFiles> {
    let json = fetch_json(&format!(
        "https://ambientcg.com/api/v2/full_json?id={}&include=downloadData",
        encode(&asset.id)
    ))?;
    let downloads = json["foundAssets"][0]["downloadFolders"]["default"]["downloadFiletypeCategories"]
        ["zip"]["downloads"]
        .as_array()
        .context("ambientCG lists no downloads for this material")?;
    let want = format!("{}-JPG", resolution.label());
    let link = downloads
        .iter()
        .find(|d| d["attribute"].as_str() == Some(want.as_str()))
        .and_then(|d| d["downloadLink"].as_str())
        .with_context(|| format!("No {want} download for {}", asset.name))?;
    let bytes = fetch(link)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .context("The download is not a ZIP archive")?;
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name()?.into_owned();
        if entry.is_dir() || material::classify(&name).is_none() {
            continue;
        }
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data)?;
        files.push((name, data));
    }
    material::from_named_files(&asset.name, files)
}

fn download_polyhaven(asset: &OnlineAsset, resolution: Resolution) -> Result<MaterialFiles> {
    let json = fetch_json(&format!("https://api.polyhaven.com/files/{}", asset.id))?;
    let res = resolution.label().to_lowercase();
    // Poly Haven's map names, and the suffix that tells the shelf the channel.
    const MAPS: [(&str, &str); 6] = [
        ("Diffuse", "diff"),
        ("Rough", "rough"),
        ("Metal", "metal"),
        ("nor_gl", "nor_gl"),
        ("Displacement", "disp"),
        ("Emission", "emission"),
    ];
    let mut files = Vec::new();
    for (key, suffix) in MAPS {
        // The smaller formats first: jpg, then png, then exr.
        let Some(url) = ["jpg", "png", "exr"]
            .iter()
            .find_map(|ext| json[key][&res][ext]["url"].as_str().map(|u| (*ext, u)))
        else {
            continue;
        };
        let bytes = fetch(url.1)?;
        files.push((format!("{}_{suffix}_{res}.{}", asset.id, url.0), bytes));
    }
    material::from_named_files(&asset.name, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_percent_encoded() {
        assert_eq!(encode("brick wall"), "brick%20wall");
        assert_eq!(encode("a-b_c.d"), "a-b_c.d");
        assert_eq!(encode("é"), "%C3%A9");
    }
}
