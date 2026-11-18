use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const WEBSITE: &str = "https://lanlink.heyyami.com";
pub const INSTALLED_VERSION: &str = match option_env!("LANLINK_RELEASE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Deserialize)]
struct ReleaseIndex {
    tag: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Update {
    Current,
    Available {
        version: String,
        download_url: String,
    },
    Unsupported,
}

pub fn check() -> Result<Update> {
    let Some(target) = package_target(std::env::consts::OS, std::env::consts::ARCH) else {
        return Ok(Update::Unsupported);
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .build()
        .into();
    let mut response = agent
        .get(format!("{WEBSITE}/release.json"))
        .header("Cache-Control", "no-cache")
        .call()?;
    ensure!(
        response.status().as_u16() == 200,
        "unexpected release status"
    );
    let body = response
        .body_mut()
        .with_config()
        .limit(1024)
        .read_to_vec()?;
    let latest: ReleaseIndex = serde_json::from_slice(&body)?;
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

fn compare(installed: &str, latest_tag: &str, target: &str) -> Result<Update> {
    let latest = parse_version(
        latest_tag
            .strip_prefix('v')
            .context("missing release tag prefix")?,
    )?;
    if latest <= parse_version(installed)? {
        return Ok(Update::Current);
    }
    let base = format!("{}.{}.{}", latest.major, latest.minor, latest.patch);
    Ok(Update::Available {
        version: latest.to_string(),
        download_url: format!("{WEBSITE}/downloads/{latest_tag}/LANLink-{base}-{target}.zip"),
    })
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
            assert_eq!(
                compare(
                    "0.2.0-preview.2",
                    "v0.2.0-preview.10",
                    package_target(os, arch).unwrap()
                )
                .unwrap(),
                Update::Available {
                    version: "0.2.0-preview.10".into(),
                    download_url: format!(
                        "{WEBSITE}/downloads/v0.2.0-preview.10/LANLink-0.2.0-{target}.zip"
                    ),
                }
            );
        }
        assert_eq!(package_target("linux", "x86_64"), None);
        let target = "aarch64-apple-darwin";
        assert!(matches!(
            compare("0.2.0-preview.10", "v0.2.0", target).unwrap(),
            Update::Available { .. }
        ));
        for (installed, published) in [
            ("0.2.0-preview.10", "v0.2.0-preview.2"),
            ("0.2.0", "v0.2.0-preview.10"),
            ("0.2.0", "v0.2.0"),
            ("0.3.0-preview.1", "v0.2.0"),
        ] {
            assert_eq!(
                compare(installed, published, target).unwrap(),
                Update::Current
            );
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
}
