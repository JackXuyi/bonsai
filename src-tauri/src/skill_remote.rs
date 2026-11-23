//! Remote skill packs: load a pack from GitHub (repo root / tree / raw link)
//! or any https `SKILL.md` URL.
//!
//! Boundary rules:
//! - https only; nothing is fetched in the background — this runs exclusively
//!   on an explicit user click;
//! - bytes are capped while reading (16KB body, 4KB × 5 refs), never after;
//! - the fetched pack is handed to `skill::install_pack_files`, so layout,
//!   injection blacklist, size caps and rollback are the same code that
//!   guards a local import (no second validator to drift).
//!
//! Supported URL forms:
//! - `https://github.com/<o>/<r>`                       → repo root, default branch
//! - `https://github.com/<o>/<r>/tree/<ref>/<path>`     → subdirectory of a branch
//! - `https://raw.githubusercontent.com/<o>/<r>/<ref>/<path>/SKILL.md`
//!   → converted into a tree fetch so `references/*.md` still come along
//! - `https://gitlab.com/<o>/<r>/-/blob/<ref>/<path>/SKILL.md` → its `/-/raw/` form
//! - any other https URL ending in `.md` → that single file, no references

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;

// Size caps are the skill module's own constants, re-exported under the
// short names this file uses — one source of truth, no drift between the
// remote pre-check and the validator that actually enforces them.
pub use crate::skill::{
    REFERENCE_MAX as MAX_REF, REFERENCES_MAX_COUNT as MAX_REFS, SKILL_MD_MAX as MAX_MD,
};
const TIMEOUT_SECS: u64 = 15;
const UA: &str = "bonsai-skill-loader";

#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Resolve through the GitHub contents API (gets refs for free).
    Github {
        owner: String,
        repo: String,
        git_ref: Option<String>,
        path: String,
    },
    /// A single markdown file at `url`.
    File(String),
}

/// Parse a user-supplied URL into a fetch plan. Every rejection is
/// `bad_skill_url` — the UI turns that into "链接形式不支持，看下面的示例".
pub fn parse(url: &str) -> Result<Plan, String> {
    let url = url.trim().trim_end_matches('/');
    let Some(rest) = url.strip_prefix("https://") else {
        return Err("bad_skill_url".into());
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match host {
        "github.com" => {
            if segs.len() < 2 {
                return Err("bad_skill_url".into());
            }
            let (owner, repo) = (segs[0].to_string(), segs[1].to_string());
            if segs.len() == 2 {
                return Ok(Plan::Github { owner, repo, git_ref: None, path: String::new() });
            }
            match (segs.get(2), segs.len()) {
                (Some(&"tree"), 4) => Ok(Plan::Github {
                    owner,
                    repo,
                    git_ref: Some(segs[3].to_string()),
                    path: String::new(),
                }),
                (Some(&"tree"), n) if n > 4 => Ok(Plan::Github {
                    owner,
                    repo,
                    git_ref: Some(segs[3].to_string()),
                    path: segs[4..].join("/"),
                }),
                _ => Err("bad_skill_url".into()),
            }
        }
        "raw.githubusercontent.com" => {
            if segs.len() >= 4 && segs.last() == Some(&"SKILL.md") {
                Ok(Plan::Github {
                    owner: segs[0].to_string(),
                    repo: segs[1].to_string(),
                    git_ref: Some(segs[2].to_string()),
                    path: segs[3..segs.len() - 1].join("/"),
                })
            } else if url.ends_with(".md") {
                Ok(Plan::File(url.to_string()))
            } else {
                Err("bad_skill_url".into())
            }
        }
        "gitlab.com" if url.contains("/-/blob/") && url.ends_with(".md") => {
            Ok(Plan::File(url.replace("/-/blob/", "/-/raw/")))
        }
        _ if url.ends_with(".md") => Ok(Plan::File(url.to_string())),
        _ => Err("bad_skill_url".into()),
    }
}

/// Derive a pack id from the plan (`<repo>` for a repo root, the folder name
/// for a subdirectory, the folder above the file otherwise).
pub fn derive_id(plan: &Plan) -> String {
    let raw = match plan {
        Plan::Github { repo, path, .. } => {
            if path.is_empty() {
                repo.clone()
            } else {
                path.rsplit('/').next().unwrap_or(repo).to_string()
            }
        }
        Plan::File(u) => {
            // Only the path half counts — never the host.
            let no_query = u.split(['?', '#']).next().unwrap_or(u);
            let path = no_query
                .split("://")
                .nth(1)
                .and_then(|rest| rest.split_once('/'))
                .map(|(_, p)| p)
                .unwrap_or(no_query);
            let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            if segs.last().is_some_and(|s| s.ends_with(".md")) {
                segs.pop();
            }
            segs.last().map(|s| s.to_string()).unwrap_or_default()
        }
    };
    sanitize_id(&raw).unwrap_or_else(|| "remote-skill".into())
}

/// lowercase a-z/0-9 with `-` separators; Anything else collapses to `-`.
fn sanitize_id(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(40).collect())
    }
}

#[derive(Debug, Deserialize)]
struct Entry {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    size: u64,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .user_agent(UA)
        .build()
}

fn map_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(404, _) => "skill_not_found".into(),
        ureq::Error::Status(403, _) | ureq::Error::Status(429, _) => "skill_rate_limited".into(),
        _ => "skill_network".into(),
    }
}

/// Percent-encode the few characters that can legally appear in a branch or
/// path but would break a query string. No extra dependency for this.
fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn get_text(agent: &ureq::Agent, url: &str, limit: usize) -> Result<String, String> {
    let resp = agent.get(url).call().map_err(map_err)?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(limit as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|_| "skill_network".to_string())?;
    if buf.len() > limit {
        return Err("skill_too_large".into());
    }
    String::from_utf8(buf).map_err(|_| "skill_invalid".to_string())
}

fn get_json(agent: &ureq::Agent, url: &str) -> Result<serde_json::Value, String> {
    let resp = agent
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(map_err)?;
    resp.into_json().map_err(|_| "skill_network".to_string())
}

fn entries(v: &serde_json::Value) -> Result<Vec<Entry>, String> {
    let arr = v.as_array().ok_or("skill_not_found")?;
    arr.iter()
        .map(|e| serde_json::from_value(e.clone()).map_err(|_| "skill_network".to_string()))
        .collect()
}

fn api_url(owner: &str, repo: &str, suffix: &str) -> String {
    format!("https://api.github.com/repos/{owner}/{repo}/{suffix}")
}

/// `contents` listing for `path` (empty = repo root) at `git_ref`.
fn contents_url(owner: &str, repo: &str, path: &str, git_ref: &str) -> String {
    let suffix = if path.is_empty() {
        format!("contents?ref={}", pct(git_ref))
    } else {
        format!("contents/{}?ref={}", pct(path), pct(git_ref))
    };
    api_url(owner, repo, &suffix)
}

/// Fetch a pack. Returns the derived id plus `(relative path, content)` pairs
/// ready for `skill::install_pack_files`.
pub fn fetch(url: &str) -> Result<(String, Vec<(String, String)>), String> {
    let plan = parse(url)?;
    let id = derive_id(&plan);
    let agent = agent();
    match plan {
        Plan::Github { owner, repo, git_ref, path } => {
            let git_ref = match git_ref {
                Some(r) => r,
                None => {
                    let v = get_json(&agent, &api_url(&owner, &repo, ""))?;
                    v.get("default_branch")
                        .and_then(|b| b.as_str())
                        .unwrap_or("main")
                        .to_string()
                }
            };
            // A URL pointing straight at the file lands on `contents/SKILL.md`
            // — the API answers with a single object, which `entries` rejects.
            // Normalise that case to its parent directory first.
            let dir_path = path.strip_suffix("/SKILL.md").unwrap_or(&path).to_string();
            let dir = entries(&get_json(
                &agent,
                &contents_url(&owner, &repo, &dir_path, &git_ref),
            )?)?;
            let skill_entry = dir
                .iter()
                .find(|e| e.name == "SKILL.md" && e.kind == "file")
                .ok_or("skill_not_found")?;
            if skill_entry.size as usize > MAX_MD {
                return Err("skill_too_large".into());
            }
            let skill_url = skill_entry
                .download_url
                .clone()
                .ok_or("skill_not_found")?;

            let mut files = vec![("SKILL.md".to_string(), get_text(&agent, &skill_url, MAX_MD)?)];

            if dir.iter().any(|e| e.name == "references" && e.kind == "dir") {
                let refs_path = if dir_path.is_empty() {
                    "references".to_string()
                } else {
                    format!("{dir_path}/references")
                };
                let refs = entries(&get_json(
                    &agent,
                    &contents_url(&owner, &repo, &refs_path, &git_ref),
                )?)?;
                let md: Vec<&Entry> = refs
                    .iter()
                    .filter(|e| e.kind == "file" && e.name.ends_with(".md"))
                    .collect();
                if md.len() > MAX_REFS {
                    return Err("skill_too_large".into());
                }
                for e in md {
                    if e.size as usize > MAX_REF {
                        return Err("skill_too_large".into());
                    }
                    let u = e.download_url.clone().ok_or("skill_not_found")?;
                    files.push((
                        format!("references/{}", e.name),
                        get_text(&agent, &u, MAX_REF)?,
                    ));
                }
            }
            Ok((id, files))
        }
        Plan::File(u) => Ok((id, vec![("SKILL.md".to_string(), get_text(&agent, &u, MAX_MD)?)])),
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_forms() {
        assert_eq!(
            parse("https://github.com/acme/skills").unwrap(),
            Plan::Github {
                owner: "acme".into(),
                repo: "skills".into(),
                git_ref: None,
                path: String::new(),
            }
        );
        assert_eq!(
            parse("https://github.com/acme/skills/tree/main/gmp").unwrap(),
            Plan::Github {
                owner: "acme".into(),
                repo: "skills".into(),
                git_ref: Some("main".into()),
                path: "gmp".into(),
            }
        );
        // raw link to SKILL.md is upgraded to a tree fetch so refs travel.
        assert_eq!(
            parse("https://raw.githubusercontent.com/acme/skills/main/gmp/SKILL.md").unwrap(),
            Plan::Github {
                owner: "acme".into(),
                repo: "skills".into(),
                git_ref: Some("main".into()),
                path: "gmp".into(),
            }
        );
    }

    #[test]
    fn parses_file_forms() {
        assert_eq!(
            parse("https://gitlab.com/acme/skills/-/blob/main/gmp/SKILL.md").unwrap(),
            Plan::File("https://gitlab.com/acme/skills/-/raw/main/gmp/SKILL.md".into())
        );
        assert_eq!(
            parse("https://example.com/packs/gmp/SKILL.md").unwrap(),
            Plan::File("https://example.com/packs/gmp/SKILL.md".into())
        );
        assert_eq!(
            parse("https://example.com/x.md").unwrap(),
            Plan::File("https://example.com/x.md".into())
        );
    }

    #[test]
    fn rejects_non_https_and_unknown_shapes() {
        for bad in [
            "http://github.com/acme/skills",
            "github.com/acme/skills",
            "https://github.com/acme",
            "https://github.com/acme/skills/blob/main/x/SKILL.md",
            "https://example.com/packs/gmp.zip",
            "",
        ] {
            assert!(matches!(parse(bad), Err(ref e) if e == "bad_skill_url"), "{bad}");
        }
    }

    #[test]
    fn derives_ids_from_each_shape() {
        let root = "https://github.com/acme/skills";
        assert_eq!(derive_id(&parse(root).unwrap()), "skills");
        let tree = "https://github.com/acme/skills/tree/main/GMP-Assistant";
        assert_eq!(derive_id(&parse(tree).unwrap()), "gmp-assistant");
        let raw = "https://raw.githubusercontent.com/acme/skills/main/gmp/SKILL.md";
        assert_eq!(derive_id(&parse(raw).unwrap()), "gmp");
        let gitlab = "https://gitlab.com/acme/skills/-/blob/main/My%20Pack/SKILL.md";
        assert_eq!(derive_id(&parse(gitlab).unwrap()), "my-20pack");
        // No usable segment → stable fallback, never an empty id.
        assert_eq!(
            derive_id(&Plan::File("https://example.com/SKILL.md".into())),
            "remote-skill"
        );
    }

    #[test]
    fn sanitize_keeps_charset_valid() {
        assert_eq!(sanitize_id("GMP__助手  v2").unwrap(), "gmp-v2");
        assert_eq!(sanitize_id("--x--").unwrap(), "x");
        assert!(sanitize_id("***").is_none());
        assert!(sanitize_id("ab").unwrap().len() <= 40);
    }

    #[test]
    fn pct_keeps_slashes_and_encodes_spaces() {
        assert_eq!(pct("skills/gmp v2"), "skills/gmp%20v2");
        assert_eq!(pct("a/b-c_d.e~f"), "a/b-c_d.e~f");
    }
}
