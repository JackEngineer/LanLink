use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    time::Duration,
};

const RELEASE_API: &str = "https://api.github.com/repos/JackEngineer/LanLink/releases/tags";
const MAX_PACKAGE: u64 = 256 * 1024 * 1024;
const WEBSITE: &str = "https://lanlink.heyyami.com";
pub const INSTALLED_VERSION: &str = match option_env!("LANLINK_RELEASE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Deserialize)]
struct ReleaseIndex {
    tag: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Update {
    Current,
    Available { version: String },
    Unsupported,
}

#[derive(Debug)]
pub struct Release {
    pub version: Version,
    target: &'static str,
}

impl Release {
    fn filename(&self) -> String {
        let Version {
            major,
            minor,
            patch,
            ..
        } = self.version;
        format!("LANLink-{major}.{minor}.{patch}-{}.zip", self.target)
    }

    pub fn download(&self, file: &mut std::fs::File) -> Result<()> {
        let tag = format!("v{}", self.version);
        let name = self.filename();
        let metadata = http(Duration::from_secs(15))
            .get(format!("{RELEASE_API}/{tag}"))
            .header("User-Agent", "LANLink")
            .header("Accept", "application/vnd.github+json")
            .call()?;
        let release: GitHubRelease = read_json(metadata, 1024 * 1024)?;
        ensure!(
            release.tag_name == tag && !release.draft && release.published_at.is_some(),
            "release is not published"
        );
        let mut assets = release
            .assets
            .into_iter()
            .filter(|asset| asset.name == name);
        let asset = assets.next().context("release package is missing")?;
        ensure!(
            assets.next().is_none()
                && asset.state == "uploaded"
                && (1..=MAX_PACKAGE).contains(&asset.size),
            "invalid release package"
        );
        let digest = asset
            .digest
            .as_deref()
            .and_then(|s| s.strip_prefix("sha256:"))
            .context("release package digest is missing")?;
        let expected: [u8; 32] = hex::decode(digest)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid package digest"))?;
        let mut response = http(Duration::from_secs(300))
            .get(format!("{WEBSITE}/downloads/{tag}/{name}"))
            .header("Accept-Encoding", "identity")
            .call()?;
        ensure!(
            response.status().as_u16() == 200,
            "unexpected package status"
        );
        let mut body = response
            .body_mut()
            .with_config()
            .limit(asset.size + 1)
            .reader();
        copy_verified(&mut body, &mut *file, asset.size, expected)?;
        file.sync_all()?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    published_at: Option<String>,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    state: String,
    size: u64,
    digest: Option<String>,
}

fn copy_verified(
    mut source: impl Read,
    mut file: impl Write,
    size: u64,
    expected: [u8; 32],
) -> Result<()> {
    let mut count = 0;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let length = source.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        count += length as u64;
        ensure!(count <= size, "unexpected package size");
        let chunk = &buffer[..length];
        digest.update(chunk);
        file.write_all(chunk)?;
    }
    ensure!(
        count == size && digest.finalize().as_slice() == expected,
        "package checksum mismatch"
    );
    Ok(())
}

fn http(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .build()
        .into()
}

fn read_json<T: serde::de::DeserializeOwned>(
    mut response: ureq::http::Response<ureq::Body>,
    limit: u64,
) -> Result<T> {
    ensure!(
        response.status().as_u16() == 200,
        "unexpected release status"
    );
    let body = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()?;
    Ok(serde_json::from_slice(&body)?)
}

pub fn latest() -> Result<Option<Release>> {
    let target = package_target(std::env::consts::OS, std::env::consts::ARCH)
        .context("unsupported update platform")?;
    let response = http(Duration::from_secs(8))
        .get(format!("{WEBSITE}/release.json"))
        .header("Cache-Control", "no-cache")
        .call()?;
    let latest: ReleaseIndex = read_json(response, 1024)?;
    compare(INSTALLED_VERSION, &latest.tag, target)
}

fn package_target(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

fn compare(installed: &str, latest_tag: &str, target: &'static str) -> Result<Option<Release>> {
    let latest = parse_version(
        latest_tag
            .strip_prefix('v')
            .context("missing release tag prefix")?,
    )?;
    Ok((latest > parse_version(installed)?).then_some(Release {
        version: latest,
        target,
    }))
}

fn parse_version(value: &str) -> Result<Version> {
    let version = Version::parse(value)?;
    let pre = version.pre.as_str();
    ensure!(
        version.build.is_empty()
            && (pre.is_empty()
                || pre.strip_prefix("preview.").is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                })),
        "unsupported release version"
    );
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_release_tags_and_uses_the_matching_zip() {
        for (os, arch, target) in [
            ("macos", "aarch64", "aarch64-apple-darwin"),
            ("macos", "x86_64", "x86_64-apple-darwin"),
            ("windows", "x86_64", "x86_64-pc-windows-msvc"),
        ] {
            let release = compare(
                "0.2.0-preview.2",
                "v0.2.0-preview.10",
                package_target(os, arch).unwrap(),
            )
            .unwrap()
            .unwrap();
            assert_eq!(release.version.to_string(), "0.2.0-preview.10");
            assert_eq!(release.filename(), format!("LANLink-0.2.0-{target}.zip"));
        }
        assert_eq!(package_target("linux", "x86_64"), None);
        let target = "aarch64-apple-darwin";
        assert!(
            compare("0.2.0-preview.10", "v0.2.0", target)
                .unwrap()
                .is_some()
        );
        for (installed, published) in [
            ("0.2.0-preview.10", "v0.2.0-preview.2"),
            ("0.2.0", "v0.2.0-preview.10"),
            ("0.2.0", "v0.2.0"),
            ("0.3.0-preview.1", "v0.2.0"),
        ] {
            assert!(compare(installed, published, target).unwrap().is_none());
        }
        for invalid in [
            "0.2.0",
            "v0.2.0-preview.01",
            "v0.2.0-preview.1.extra",
            "v0.2.0+other",
            "v0.2.0/../../package",
        ] {
            assert!(compare("0.1.0", invalid, target).is_err());
        }
    }

    #[test]
    fn rejects_truncated_modified_and_oversized_packages() {
        let official = b"LANLink package";
        let digest = Sha256::digest(official).into();
        for invalid in [&official[..4], b"LONLink package", b"LANLink package plus"] {
            assert!(copy_verified(invalid, Vec::new(), official.len() as u64, digest).is_err());
        }
        let mut file = Vec::new();
        copy_verified(
            official.as_slice(),
            &mut file,
            official.len() as u64,
            digest,
        )
        .unwrap();
        assert_eq!(file, official);
    }
}
