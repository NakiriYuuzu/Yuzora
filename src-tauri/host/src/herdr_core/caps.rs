//! Capability and version gate: what Yuzora needs from a running core, decided only from
//! `hello` (version, generation, methods, features) / `rpc.discover`, never by probing.

use std::collections::BTreeSet;

use serde_json::Value;

/// Protocol generation this client speaks (sent in `hello`).
pub const GENERATION: u32 = 1;
/// The oldest core version whose behavior is recorded and replayed. Below it: refuse.
pub const MIN_VERSION: &str = "0.1.11";
/// The version Yuzora bundles (`src-tauri/herdr-core-runtime.json`). A running core newer than
/// this is fine: the API only grows.
pub const BUNDLED_VERSION: &str = "0.1.14";
/// Methods the experimental runtime cannot work without.
pub const REQUIRED_METHODS: &[&str] = &[
    "hello",
    "server.status",
    "terminal.list",
    "terminal.run",
    "terminal.attach",
    "terminal.kill",
    "events.subscribe",
    "doc.get",
    "doc.set",
];
/// Used when present; the UI hides what they power otherwise.
pub const OPTIONAL_METHODS: &[&str] = &[
    "terminal.keep",
    "terminal.read",
    "terminal.rm",
    "terminal.label",
    "machine.list",
    "rpc.discover",
];
pub const REQUIRED_FEATURES: &[&str] = &["framed", "attach", "events", "status"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub version: String,
    pub generation: u32,
    pub methods: BTreeSet<String>,
    pub features: BTreeSet<String>,
    pub core_id: String,
    pub framed: bool,
}

impl Hello {
    /// From a `hello` result object.
    pub fn parse(result: &Value) -> Result<Hello, String> {
        let strings = |key: &str| -> Result<BTreeSet<String>, String> {
            result
                .get(key)
                .and_then(Value::as_array)
                .ok_or(format!("hello: no {key}"))?
                .iter()
                .map(|m| {
                    m.as_str()
                        .map(String::from)
                        .ok_or(format!("hello: non-string in {key}"))
                })
                .collect()
        };
        Ok(Hello {
            version: result
                .get("version")
                .and_then(Value::as_str)
                .ok_or("hello: no version")?
                .to_string(),
            generation: result
                .get("generation")
                .and_then(Value::as_u64)
                .and_then(|g| u32::try_from(g).ok())
                .ok_or("hello: no generation")?,
            methods: strings("methods")?,
            features: strings("features")?,
            core_id: result
                .pointer("/core/id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            framed: result
                .get("framed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    pub fn has(&self, method: &str) -> bool {
        self.methods.contains(method)
    }
}

/// Method names in an OpenRPC document (`rpc.discover` result or `hc schema`).
pub fn openrpc_methods(doc: &Value) -> BTreeSet<String> {
    doc.get("methods")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("name").and_then(Value::as_str).map(String::from))
        .collect()
}

pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches("hc ").trim_start_matches('v');
    let mut parts = v
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    Same,
    /// The running core is newer than the bundled copy (fine: the API only grows).
    CoreNewer,
    /// Older than bundled but not below [`MIN_VERSION`].
    CoreOlder,
}

/// Why a running core is not used. Stable `code()`s; the UI text lives in the frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The core speaks another protocol generation.
    Generation {
        core: u32,
    },
    MissingMethods(Vec<String>),
    MissingFeatures(Vec<String>),
    /// Below [`MIN_VERSION`].
    TooOld {
        version: String,
    },
    UnparsedVersion(String),
}

impl Refusal {
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::Generation { .. } => "generation",
            Refusal::MissingMethods(_) => "missing_methods",
            Refusal::MissingFeatures(_) => "missing_features",
            Refusal::TooOld { .. } => "too_old",
            Refusal::UnparsedVersion(_) => "unparsed_version",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Generation { core } => {
                write!(
                    f,
                    "core speaks protocol generation {core}, Yuzora {GENERATION}"
                )
            }
            Refusal::MissingMethods(m) => write!(f, "core lacks methods {}", m.join(", ")),
            Refusal::MissingFeatures(m) => write!(f, "core lacks features {}", m.join(", ")),
            Refusal::TooOld { version } => {
                write!(f, "core {version} is older than the minimum {MIN_VERSION}")
            }
            Refusal::UnparsedVersion(v) => write!(f, "unparsed core version {v:?}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Use {
        relation: Relation,
        optional_missing: Vec<&'static str>,
    },
    Refuse(Refusal),
}

/// Decides whether Yuzora may use the core that answered `hello`. Never decides to restart or
/// replace it: a refused core is reported to the user, not touched.
pub fn gate(h: &Hello, bundled: &str) -> Verdict {
    if h.generation != GENERATION {
        return Verdict::Refuse(Refusal::Generation { core: h.generation });
    }
    let missing: Vec<String> = REQUIRED_METHODS
        .iter()
        .filter(|m| !h.has(m))
        .map(|m| m.to_string())
        .collect();
    if !missing.is_empty() {
        return Verdict::Refuse(Refusal::MissingMethods(missing));
    }
    let missing_features: Vec<String> = REQUIRED_FEATURES
        .iter()
        .filter(|f| !h.features.contains(**f))
        .map(|f| f.to_string())
        .collect();
    if !missing_features.is_empty() {
        return Verdict::Refuse(Refusal::MissingFeatures(missing_features));
    }
    let (Some(core), Some(ours), Some(min)) = (
        parse_version(&h.version),
        parse_version(bundled),
        parse_version(MIN_VERSION),
    ) else {
        return Verdict::Refuse(Refusal::UnparsedVersion(h.version.clone()));
    };
    if core < min {
        return Verdict::Refuse(Refusal::TooOld {
            version: h.version.clone(),
        });
    }
    let relation = match core.cmp(&ours) {
        std::cmp::Ordering::Equal => Relation::Same,
        std::cmp::Ordering::Greater => Relation::CoreNewer,
        std::cmp::Ordering::Less => Relation::CoreOlder,
    };
    Verdict::Use {
        relation,
        optional_missing: OPTIONAL_METHODS
            .iter()
            .copied()
            .filter(|m| !h.has(m))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr_core::fixtures;

    fn recorded_hello(version: &str) -> Hello {
        Hello::parse(&fixtures::json(&format!("v{version}/hello.json"))["result"]).unwrap()
    }

    #[test]
    fn recorded_hello_and_discover_agree_for_every_pinned_version() {
        for v in fixtures::VERSIONS {
            let hello = recorded_hello(v);
            assert_eq!(hello.version, *v);
            assert_eq!(hello.generation, 1);
            let discover = fixtures::json(&format!("v{v}/rpc.discover.json"));
            let from_doc = openrpc_methods(&discover["result"]);
            // hello lists every OpenRPC method (rpc.discover itself included).
            assert_eq!(
                hello.methods, from_doc,
                "v{v}: hello.methods vs rpc.discover"
            );
            let methods = fixtures::json(&format!("v{v}/methods.json"));
            assert_eq!(methods["generation"], 1);
            let listed: BTreeSet<String> = methods["methods"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m.as_str().unwrap().to_string())
                .collect();
            assert_eq!(listed, hello.methods);
        }
    }

    #[test]
    fn bundled_version_matches_the_pin() {
        let pins = fixtures::json("pins.json");
        assert_eq!(pins["bundled"]["version"], BUNDLED_VERSION);
        assert_eq!(fixtures::BUNDLED, BUNDLED_VERSION);
    }

    #[test]
    fn released_openrpc_is_a_subset_of_every_recorded_core() {
        // The API only grows: a core at the pin (or newer) answers every released method.
        let released = openrpc_methods(&fixtures::json("released.openrpc.json"));
        assert!(released.len() > 20);
        let pinned = recorded_hello(fixtures::BUNDLED);
        for m in &released {
            assert!(
                pinned.has(m),
                "released method {m} missing from the pinned core"
            );
        }
    }

    #[test]
    fn gate_accepts_recorded_cores_and_refuses_bad_ones() {
        let pinned = recorded_hello(fixtures::BUNDLED);
        let older = recorded_hello(fixtures::OLDER);
        assert!(matches!(
            gate(&pinned, BUNDLED_VERSION),
            Verdict::Use {
                relation: Relation::Same,
                ..
            }
        ));
        assert!(matches!(
            gate(&older, BUNDLED_VERSION),
            Verdict::Use {
                relation: Relation::CoreOlder,
                ..
            }
        ));
        // The bundled copy older than a running user core: use the newer core.
        assert!(matches!(
            gate(&pinned, fixtures::OLDER),
            Verdict::Use {
                relation: Relation::CoreNewer,
                ..
            }
        ));
        let mut g2 = pinned.clone();
        g2.generation = 2;
        assert_eq!(
            gate(&g2, BUNDLED_VERSION),
            Verdict::Refuse(Refusal::Generation { core: 2 })
        );
        let mut no_docs = pinned.clone();
        no_docs.methods.remove("doc.set");
        assert_eq!(
            gate(&no_docs, BUNDLED_VERSION),
            Verdict::Refuse(Refusal::MissingMethods(vec!["doc.set".into()]))
        );
        let mut no_framed = pinned.clone();
        no_framed.features.remove("framed");
        assert_eq!(
            gate(&no_framed, BUNDLED_VERSION),
            Verdict::Refuse(Refusal::MissingFeatures(vec!["framed".into()]))
        );
        let mut ancient = pinned.clone();
        ancient.version = "0.1.5".into();
        assert_eq!(
            gate(&ancient, BUNDLED_VERSION),
            Verdict::Refuse(Refusal::TooOld {
                version: "0.1.5".into()
            })
        );
        let mut odd = pinned;
        odd.version = "banana".into();
        assert_eq!(
            gate(&odd, BUNDLED_VERSION),
            Verdict::Refuse(Refusal::UnparsedVersion("banana".into()))
        );
    }

    #[test]
    fn optional_methods_are_reported_not_required() {
        let mut pinned = recorded_hello(fixtures::BUNDLED);
        pinned.methods.remove("terminal.keep");
        match gate(&pinned, BUNDLED_VERSION) {
            Verdict::Use {
                optional_missing, ..
            } => assert_eq!(optional_missing, ["terminal.keep"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn refusals_have_stable_codes() {
        let codes: Vec<&str> = [
            Refusal::Generation { core: 2 },
            Refusal::MissingMethods(vec![]),
            Refusal::MissingFeatures(vec![]),
            Refusal::TooOld {
                version: "0".into(),
            },
            Refusal::UnparsedVersion("x".into()),
        ]
        .iter()
        .map(Refusal::code)
        .collect();
        assert_eq!(
            codes,
            [
                "generation",
                "missing_methods",
                "missing_features",
                "too_old",
                "unparsed_version"
            ]
        );
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("0.1.13") > parse_version("0.1.9"));
        assert_eq!(parse_version("hc 0.1.11"), Some((0, 1, 11)));
        assert_eq!(parse_version("v0.2.0-rc1"), Some((0, 2, 0)));
        assert_eq!(parse_version("x"), None);
    }
}
