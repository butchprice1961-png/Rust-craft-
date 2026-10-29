//! Version predicates as Fabric evaluates them in `fabric.mod.json`:
//! semantic versions (`1.20.1`, `0.15.11+build.3`, `1.21-alpha.24.33.a`)
//! against terms like `>=1.20 <1.21`, `~1.20.1`, `^0.15`, `1.20.x` or `*`.

use std::cmp::Ordering;

#[derive(Debug, Clone)]
pub struct Version {
    core: Vec<u64>,
    pre: Option<Vec<String>>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        // Build metadata never affects ordering.
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let core: Vec<u64> = core.split('.').map(|c| c.parse().ok()).collect::<Option<_>>()?;
        let pre = match pre {
            Some(p) => {
                let ids: Vec<String> = p.split('.').map(String::from).collect();
                if ids.iter().any(String::is_empty) {
                    return None;
                }
                Some(ids)
            }
            None => None,
        };
        Some(Version { core, pre })
    }
}

/// Missing components count as 0, so `1.20` equals `1.20.0`.
fn cmp_core(a: &[u64], b: &[u64]) -> Ordering {
    (0..a.len().max(b.len()))
        .map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)))
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}

fn cmp_ident(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        _ => a.cmp(b),
    }
}

/// Semantic-version order: a pre-release sorts before its release.
pub fn compare(a: &Version, b: &Version) -> Ordering {
    cmp_core(&a.core, &b.core).then_with(|| match (&a.pre, &b.pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            x.iter().zip(y).map(|(p, q)| cmp_ident(p, q)).find(|o| o.is_ne()).unwrap_or_else(|| x.len().cmp(&y.len()))
        }
    })
}

/// `core` with component `i` raised by one and everything after dropped.
fn bump(core: &[u64], i: usize) -> Vec<u64> {
    let mut out: Vec<u64> = (0..=i).map(|k| core.get(k).copied().unwrap_or(0)).collect();
    out[i] += 1;
    out
}

fn term(t: &str, v: &Version) -> Option<bool> {
    if t == "*" {
        return Some(true);
    }
    let (op, rest) = ["<=", ">=", "<", ">", "=", "~", "^"]
        .iter()
        .find_map(|op| t.strip_prefix(op).map(|r| (*op, r)))
        .unwrap_or(("=", t));
    let parts: Vec<&str> = rest.split(['-', '+']).next()?.split('.').collect();
    if let Some(i) = parts.iter().position(|p| matches!(*p, "x" | "X" | "*")) {
        // `1.20.x`: the components before the wildcard must match.
        if op != "=" || i == 0 {
            return None;
        }
        let base: Vec<u64> = parts[..i].iter().map(|p| p.parse().ok()).collect::<Option<_>>()?;
        return Some(base.iter().enumerate().all(|(k, b)| v.core.get(k).unwrap_or(&0) == b));
    }
    let b = Version::parse(rest)?;
    let c = compare(v, &b);
    // Upper bounds compare cores only, so `~1.20` excludes `1.21-alpha.1`.
    let below = |limit: Vec<u64>| cmp_core(&v.core, &limit).is_lt();
    Some(match op {
        ">=" => c.is_ge(),
        "<=" => c.is_le(),
        ">" => c.is_gt(),
        "<" => c.is_lt(),
        "=" => c.is_eq(),
        "~" => c.is_ge() && below(bump(&b.core, 1.min(b.core.len() - 1))),
        _ => c.is_ge() && below(bump(&b.core, 0)),
    })
}

/// Whether `version` satisfies any of `predicates` (each a space-separated
/// list of terms that must all hold). `None` when either side is not
/// something Fabric could compare either, such as a non-semantic version.
pub fn satisfies(predicates: &[String], version: &str) -> Option<bool> {
    if predicates.is_empty() {
        return Some(true);
    }
    let Some(v) = Version::parse(version) else {
        // Fabric compares non-semantic versions only for equality.
        let any =
            predicates.iter().any(|p| p.split_whitespace().all(|t| t == "*" || t.trim_start_matches('=') == version));
        let only_equality =
            predicates.iter().flat_map(|p| p.split_whitespace()).all(|t| !t.starts_with(['<', '>', '~', '^']));
        return if only_equality { Some(any) } else { any.then_some(true) };
    };
    let mut unknown = false;
    for p in predicates {
        let terms: Option<Vec<bool>> = p.split_whitespace().map(|t| term(t, &v)).collect();
        match terms {
            Some(t) if t.iter().all(|&ok| ok) => return Some(true),
            Some(_) => {}
            None => unknown = true,
        }
    }
    (!unknown).then_some(false)
}

/// Minecraft's version id in the form Fabric compares: releases as they
/// are, pre-releases and release candidates as `-beta.N` / `-rc.N`.
/// Weekly snapshots (`24w33a`) need Fabric's lookup table: `None`.
pub fn minecraft(id: &str) -> Option<String> {
    let (base, tag) = match id.split_once('-') {
        None => return Version::parse(id).map(|_| id.to_string()),
        Some(split) => split,
    };
    Version::parse(base)?;
    let n = |prefix: &str| tag.strip_prefix(prefix).and_then(|n| n.parse::<u32>().ok());
    if let Some(n) = n("pre") { Some(format!("{base}-beta.{n}")) } else { n("rc").map(|n| format!("{base}-rc.{n}")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(pred: &str, v: &str) -> Option<bool> {
        satisfies(&[pred.to_string()], v)
    }

    #[test]
    fn ordering_follows_semver() {
        let v = |s| Version::parse(s).unwrap();
        assert!(compare(&v("1.20"), &v("1.20.0")).is_eq());
        assert!(compare(&v("1.20.1"), &v("1.20.1+build.5")).is_eq());
        assert!(compare(&v("1.21-alpha.24.33.a"), &v("1.21")).is_lt());
        assert!(compare(&v("1.0.0-beta.2"), &v("1.0.0-beta.11")).is_lt());
        assert!(compare(&v("1.0.0-alpha"), &v("1.0.0-1")).is_gt());
        assert!(Version::parse("v2").is_none() && Version::parse("1.0-").is_none());
    }

    #[test]
    fn predicates_match_fabric_rules() {
        assert_eq!(ok("*", "1.8.9"), Some(true));
        assert_eq!(ok(">=1.20 <1.21", "1.20.4"), Some(true));
        assert_eq!(ok(">=1.20 <1.21", "1.21"), Some(false));
        assert_eq!(ok("~1.20.1", "1.20.6"), Some(true));
        assert_eq!(ok("~1.20.1", "1.20"), Some(false));
        assert_eq!(ok("~1.20", "1.21-alpha.24.33.a"), Some(false));
        // Fabric's caret keeps the major version, even 0.
        assert_eq!(ok("^0.15", "0.16.10"), Some(true));
        assert_eq!(ok("^0.15", "1.0.0"), Some(false));
        assert_eq!(ok("^0.15", "0.14.9"), Some(false));
        assert_eq!(ok("1.21.x", "1.21.4"), Some(true));
        assert_eq!(ok("1.21.x", "1.20.6"), Some(false));
        assert_eq!(ok("1.21.4", "1.21.4"), Some(true));
        assert_eq!(ok("=1.21.4", "1.21.3"), Some(false));
        assert_eq!(satisfies(&["1.20.1".into(), "1.20.4".into()], "1.20.4"), Some(true));
        assert_eq!(ok(">=x", "1.0"), None);
        // Non-semantic versions: only equality or "*".
        assert_eq!(ok("*", "beta-7"), Some(true));
        assert_eq!(ok("beta-7", "beta-7"), Some(true));
        assert_eq!(ok("beta-8", "beta-7"), Some(false));
        assert_eq!(ok(">=1.0", "beta-7"), None);
    }

    #[test]
    fn minecraft_ids_are_normalised() {
        assert_eq!(minecraft("1.21.4").as_deref(), Some("1.21.4"));
        assert_eq!(minecraft("1.20.5-pre1").as_deref(), Some("1.20.5-beta.1"));
        assert_eq!(minecraft("1.16-rc1").as_deref(), Some("1.16-rc.1"));
        assert_eq!(minecraft("24w33a"), None);
        assert_eq!(ok(">=1.20.5", &minecraft("1.20.5-pre1").unwrap()), Some(false));
    }
}
