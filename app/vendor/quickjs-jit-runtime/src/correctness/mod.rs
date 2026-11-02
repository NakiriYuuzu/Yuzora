//! Shared correctness-suite parsing and reporting primitives.
//!
//! The command-line runners and integration tests use this module so a suite
//! cannot silently interpret Test262 metadata differently in different modes.

use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt, fs, io,
    ops::Range,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ParseError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NegativePhase {
    Parse,
    Resolution,
    Runtime,
}

#[derive(Clone, Debug, Deserialize)]
struct NegativeMetadata {
    phase: NegativePhase,
    #[serde(rename = "type")]
    error_type: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Metadata {
    #[serde(default)]
    flags: Vec<String>,
    #[serde(default)]
    includes: Vec<String>,
    #[serde(default)]
    features: Vec<String>,
    negative: Option<NegativeMetadata>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Test262Variant {
    SloppyScript,
    StrictScript,
    RawScript,
    RawModule,
    Module,
    StrictModule,
    AsyncScript,
    StrictAsyncScript,
    ModuleAsync,
    StrictModuleAsync,
}

#[derive(Clone, Debug)]
pub struct Negative {
    phase: NegativePhase,
    error_type: String,
}

impl Negative {
    pub const fn phase(&self) -> NegativePhase {
        self.phase
    }

    pub fn error_type(&self) -> &str {
        &self.error_type
    }
}

#[derive(Clone, Debug)]
pub struct Test262Case {
    path: String,
    source: String,
    metadata_range: Range<usize>,
    body_range: Range<usize>,
    body: String,
    includes: Vec<String>,
    features: Vec<String>,
    negative: Option<Negative>,
    variants: Vec<Test262Variant>,
}

impl Test262Case {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn metadata_range(&self) -> Range<usize> {
        self.metadata_range.clone()
    }
    pub fn body_range(&self) -> Range<usize> {
        self.body_range.clone()
    }
    pub fn body(&self) -> &str {
        &self.body
    }
    pub fn includes(&self) -> &[String] {
        &self.includes
    }
    pub fn features(&self) -> &[String] {
        &self.features
    }
    pub fn negative(&self) -> Option<&Negative> {
        self.negative.as_ref()
    }
    pub fn variants(&self) -> &[Test262Variant] {
        &self.variants
    }
}

pub fn parse_test262(path: impl Into<String>, source: &str) -> Result<Test262Case, ParseError> {
    let path = path.into();
    let marker = "/*---";
    let start = source
        .find(marker)
        .ok_or_else(|| ParseError(format!("{path}: missing Test262 YAML frontmatter")))?;
    let yaml_start = start + marker.len();
    let relative_end = source[yaml_start..]
        .find("---*/")
        .ok_or_else(|| ParseError(format!("{path}: unterminated Test262 YAML frontmatter")))?;
    let yaml_end = yaml_start + relative_end;
    let metadata: Metadata = serde_yaml::from_str(&source[yaml_start..yaml_end])
        .map_err(|error| ParseError(format!("{path}: invalid Test262 YAML: {error}")))?;
    const KNOWN_FLAGS: &[&str] = &[
        "onlyStrict",
        "noStrict",
        "raw",
        "async",
        "module",
        "CanBlockIsFalse",
        "CanBlockIsTrue",
        "generated",
    ];
    for flag in &metadata.flags {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(ParseError(format!("{path}: unknown flag {flag}")));
        }
    }
    let has = |flag: &str| metadata.flags.iter().any(|candidate| candidate == flag);
    if has("onlyStrict") && has("noStrict")
        || has("raw")
            && metadata
                .flags
                .iter()
                .any(|flag| flag != "raw" && flag != "module")
    {
        return Err(ParseError(format!("{path}: contradictory Test262 flags")));
    }
    let variants = if has("raw") {
        vec![if has("module") {
            Test262Variant::RawModule
        } else {
            Test262Variant::RawScript
        }]
    } else {
        let module = has("module");
        let asynchronous = has("async");
        let variant = |strict| match (module, asynchronous, strict) {
            (false, false, false) => Test262Variant::SloppyScript,
            (false, false, true) => Test262Variant::StrictScript,
            (true, false, false) => Test262Variant::Module,
            (true, false, true) => Test262Variant::StrictModule,
            (false, true, false) => Test262Variant::AsyncScript,
            (false, true, true) => Test262Variant::StrictAsyncScript,
            (true, true, false) => Test262Variant::ModuleAsync,
            (true, true, true) => Test262Variant::StrictModuleAsync,
        };
        if has("onlyStrict") || module {
            vec![variant(true)]
        } else if has("noStrict") {
            vec![variant(false)]
        } else {
            vec![variant(false), variant(true)]
        }
    };
    let metadata_end = yaml_end + "---*/".len();
    let body_start = if source[metadata_end..].starts_with("\r\n") {
        metadata_end + 2
    } else if source[metadata_end..].starts_with('\n') {
        metadata_end + 1
    } else {
        metadata_end
    };
    let body = source[body_start..].to_owned();
    Ok(Test262Case {
        path,
        source: source.to_owned(),
        metadata_range: start..metadata_end,
        body_range: body_start..source.len(),
        body,
        includes: metadata.includes,
        features: metadata.features,
        negative: metadata.negative.map(|negative| Negative {
            phase: negative.phase,
            error_type: negative.error_type,
        }),
        variants,
    })
}

pub fn compose_test262_program(
    case: &Test262Case,
    variant: Test262Variant,
    harness: &str,
) -> String {
    if matches!(
        variant,
        Test262Variant::RawScript | Test262Variant::RawModule
    ) {
        // Test262 `raw` means the source text is evaluated verbatim.  The
        // frontmatter is a JavaScript comment; stripping it would also strip
        // license text, hashbang positioning, or any Annex B lexical sentinel
        // which precedes it.
        return case.source().to_owned();
    }
    let mut program = String::with_capacity(harness.len() + case.body().len() + 16);
    program.push_str(harness);
    if !harness.is_empty() && !harness.ends_with('\n') {
        program.push('\n');
    }
    if matches!(
        variant,
        Test262Variant::StrictScript | Test262Variant::StrictAsyncScript
    ) {
        program.push_str("'use strict';\n");
    }
    program.push_str(case.body());
    program
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusion {
    pub pattern: String,
    pub reason: String,
    pub owner: String,
    pub expires: String,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ExclusionManifest(Vec<Exclusion>);

impl ExclusionManifest {
    pub fn new(entries: Vec<Exclusion>) -> Self {
        Self(entries)
    }

    pub fn validate(&self, today: &str) -> Result<(), String> {
        for entry in &self.0 {
            if entry.pattern.trim().is_empty()
                || entry.reason.trim().is_empty()
                || entry.owner.trim().is_empty()
            {
                return Err("exclusion pattern, reason, and owner must be nonempty".into());
            }
            if !valid_iso_date(&entry.expires) {
                return Err(format!(
                    "{} has invalid expiry {}",
                    entry.pattern, entry.expires
                ));
            }
            if entry.expires.as_str() < today {
                return Err(format!(
                    "{} exclusion expired on {}",
                    entry.pattern, entry.expires
                ));
            }
        }
        Ok(())
    }

    pub fn reason_for(&self, path: &str, features: &[String]) -> Option<String> {
        self.0
            .iter()
            .find(|entry| {
                let path_matches = entry.pattern == "**"
                    || entry.pattern == path
                    || entry
                        .pattern
                        .strip_suffix("**")
                        .is_some_and(|prefix| path.starts_with(prefix));
                path_matches
                    && (entry.features.is_empty()
                        || entry.features.iter().any(|feature| {
                            features.iter().any(|candidate| {
                                candidate == feature
                                    || candidate
                                        .strip_prefix(feature)
                                        .is_some_and(|suffix| suffix.starts_with('.'))
                            })
                        }))
            })
            .map(|entry| {
                format!(
                    "{} (owner: {}, expires: {})",
                    entry.reason, entry.owner, entry.expires
                )
            })
    }
    pub fn path_reason(&self, path: &str) -> Option<String> {
        self.0
            .iter()
            .find(|entry| {
                entry.features.is_empty()
                    && entry.pattern != "**"
                    && (entry.pattern == path
                        || entry
                            .pattern
                            .strip_suffix("**")
                            .is_some_and(|prefix| path.starts_with(prefix)))
            })
            .map(|entry| {
                format!(
                    "{} (owner: {}, expires: {})",
                    entry.reason, entry.owner, entry.expires
                )
            })
    }
}

fn valid_iso_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    Interpreter,
    Automatic,
    ForceTier1,
    ForceTier2,
}

impl std::str::FromStr for RunMode {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "interpreter" => Ok(Self::Interpreter),
            "automatic" => Ok(Self::Automatic),
            "force-tier1" | "force-tier1-eligible" => Ok(Self::ForceTier1),
            "force-tier2" | "force-tier2-eligible" => Ok(Self::ForceTier2),
            _ => Err(format!("unknown strict runtime mode {value}")),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Pass,
    Fail,
    Skip,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NativeEvidence {
    pub native_entries: u64,
    pub tier2_entries: u64,
    pub native_exits: u64,
    pub unexpected_fallbacks: u64,
    pub opcode_ids: Vec<u16>,
    pub helper_ids: Vec<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaseReport {
    pub path: String,
    pub variant: Test262Variant,
    pub status: CaseStatus,
    pub duration_ms: u64,
    pub negative_phase: Option<NegativePhase>,
    pub negative_type: Option<String>,
    pub skip_reason: Option<String>,
    pub error: Option<String>,
    pub native: NativeEvidence,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SuiteReport {
    pub schema_version: u32,
    pub suite: String,
    pub suite_revision: String,
    pub opcode_fingerprint: u64,
    pub mode: RunMode,
    pub discovered_files: usize,
    pub shard_index: usize,
    pub shard_count: usize,
    pub cases: Vec<CaseReport>,
}

impl SuiteReport {
    pub fn new(
        suite: impl Into<String>,
        suite_revision: impl Into<String>,
        mode: RunMode,
        cases: Vec<CaseReport>,
    ) -> Self {
        Self {
            schema_version: 1,
            suite: suite.into(),
            suite_revision: suite_revision.into(),
            opcode_fingerprint: crate::abi::OPCODE_FINGERPRINT,
            mode,
            discovered_files: cases.len(),
            shard_index: 0,
            shard_count: 1,
            cases,
        }
    }
    pub fn with_discovery(
        mut self,
        discovered_files: usize,
        shard_index: usize,
        shard_count: usize,
    ) -> Self {
        self.discovered_files = discovered_files;
        self.shard_index = shard_index;
        self.shard_count = shard_count;
        self
    }

    pub fn merge_shards(mut shards: Vec<Self>) -> Result<Self, String> {
        if shards.is_empty() {
            return Err("cannot merge zero shard reports".into());
        }
        let expected = shards[0].shard_count;
        if expected == 0 {
            return Err("invalid zero shard count".into());
        }
        let mut seen = vec![false; expected];
        for shard in &shards {
            if shard.shard_count != expected
                || shard.suite != shards[0].suite
                || shard.suite_revision != shards[0].suite_revision
                || shard.mode != shards[0].mode
                || shard.opcode_fingerprint != shards[0].opcode_fingerprint
            {
                return Err("incompatible shard report metadata".into());
            }
            if shard.cases.is_empty() {
                return Err(format!("shard {} contains zero cases", shard.shard_index));
            }
            if shard.shard_index >= expected {
                return Err(format!("invalid shard index {}", shard.shard_index));
            }
            if std::mem::replace(&mut seen[shard.shard_index], true) {
                return Err(format!("duplicate shard {}", shard.shard_index));
            }
        }
        if let Some(index) = seen.iter().position(|present| !present) {
            return Err(format!("missing shard {index}"));
        }
        shards.sort_by_key(|report| report.shard_index);
        let mut merged = shards.remove(0);
        for shard in shards {
            merged.cases.extend(shard.cases);
        }
        merged.shard_index = 0;
        merged.shard_count = 1;
        merged.validate()?;
        Ok(merged)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.cases.is_empty() {
            return Err(format!("{} report contains zero cases", self.suite));
        }
        if self.discovered_files == 0
            || self.shard_count == 0
            || self.shard_index >= self.shard_count
        {
            return Err("invalid discovery/shard metadata".into());
        }
        if matches!(self.mode, RunMode::ForceTier1 | RunMode::ForceTier2) {
            for case in self
                .cases
                .iter()
                .filter(|case| matches!(case.status, CaseStatus::Pass))
            {
                if case.native.native_entries == 0 {
                    return Err(format!(
                        "{} {:?} passed forced mode without native entry",
                        case.path, case.variant
                    ));
                }
                if self.mode == RunMode::ForceTier2 && case.native.tier2_entries == 0 {
                    return Err(format!(
                        "{} {:?} passed forced Tier2 without Tier2 entry",
                        case.path, case.variant
                    ));
                }
                if case.native.unexpected_fallbacks != 0 {
                    return Err(format!(
                        "{} {:?} had unexpected fallback",
                        case.path, case.variant
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeatureDisposition {
    Supported,
    Unsupported(Vec<String>),
}

/// Classifies the stable host features needed by focused/replay tests.
/// Full-suite runners additionally load the pinned corpus feature registry;
/// a spelling absent from both sources is a configuration error, never a skip.
pub fn classify_features(features: &[String]) -> Result<FeatureDisposition, String> {
    const SUPPORTED: &[&str] = &[
        "ArrayBuffer",
        "BigInt",
        "DataView",
        "Map",
        "Proxy",
        "Reflect",
        "Set",
        "SharedArrayBuffer",
        "Symbol",
        "TypedArray",
        "WeakMap",
        "WeakRef",
    ];
    const UNSUPPORTED: &[&str] = &[
        "agent",
        "Atomics",
        "IsHTMLDDA",
        "Intl",
        "ShadowRealm",
        "Temporal",
    ];
    let mut unsupported = Vec::new();
    for feature in features {
        if SUPPORTED.contains(&feature.as_str()) {
            continue;
        }
        if UNSUPPORTED.contains(&feature.as_str()) {
            unsupported.push(feature.clone());
            continue;
        }
        return Err(format!("unknown Test262 feature {feature}"));
    }
    if unsupported.is_empty() {
        Ok(FeatureDisposition::Supported)
    } else {
        Ok(FeatureDisposition::Unsupported(unsupported))
    }
}

pub fn classify_features_against(
    features: &[String],
    registry: &str,
) -> Result<FeatureDisposition, String> {
    let known: std::collections::BTreeSet<_> = registry
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let mut unsupported = Vec::new();
    for feature in features {
        if !known.contains(feature.as_str()) {
            return Err(format!("unknown Test262 feature {feature}"));
        }
        if feature == "IsHTMLDDA"
            || feature == "agent"
            || feature.starts_with("Intl")
            || matches!(feature.as_str(), "ShadowRealm" | "Temporal")
        {
            unsupported.push(feature.clone());
        }
    }
    if unsupported.is_empty() {
        Ok(FeatureDisposition::Supported)
    } else {
        Ok(FeatureDisposition::Unsupported(unsupported))
    }
}

pub fn classify_features_with_config(
    features: &[String],
    registry: &str,
    config: &str,
) -> Result<FeatureDisposition, String> {
    let mut known: std::collections::BTreeSet<_> = registry
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let mut in_features = false;
    let mut skipped = std::collections::BTreeSet::new();
    for raw in config.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_features = line == "[features]";
            continue;
        }
        if !in_features || line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once('=').unwrap_or((line, ""));
        known.insert(name.trim());
        if value.trim().starts_with("skip") {
            skipped.insert(name.trim());
        }
    }
    let mut unsupported = Vec::new();
    for feature in features {
        if !known.contains(feature.as_str()) {
            return Err(format!("unknown Test262 feature {feature}"));
        }
        if skipped.contains(feature.as_str())
            || matches!(
                feature.as_str(),
                "IsHTMLDDA"
                    | "agent"
                    | "Atomics"
                    | "cross-realm"
                    | "json-modules"
                    | "import-text"
                    | "import-bytes"
            )
        {
            unsupported.push(feature.clone());
        }
    }
    if unsupported.is_empty() {
        Ok(FeatureDisposition::Supported)
    } else {
        Ok(FeatureDisposition::Unsupported(unsupported))
    }
}

pub fn discover_test262(root: &Path) -> io::Result<Vec<PathBuf>> {
    fn visit(path: &Path, output: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit(&path, output)?;
            } else if path.extension().is_some_and(|extension| extension == "js") {
                output.push(path);
            }
        }
        Ok(())
    }
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Test262 root {} is not initialized", root.display()),
        ));
    }
    let mut cases = Vec::new();
    visit(root, &mut cases)?;
    cases.sort();
    if cases.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("zero Test262 cases under {}", root.display()),
        ));
    }
    Ok(cases)
}

/// Returns whether the pinned QuickJS Test262 configuration excludes `path`.
///
/// The upstream runner accepts exact file entries and directory prefixes in
/// `[exclude]`, with later `!` entries re-including a subtree. Keeping this
/// interpretation here makes the Rust-host corpus match the authoritative
/// QuickJS baseline without silently turning engine-known failures into JIT
/// regressions.
pub fn quickjs_config_excludes_path(config: &str, path: &str) -> bool {
    let mut in_exclude = false;
    let mut excluded = false;
    for raw in config.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_exclude = line == "[exclude]";
            continue;
        }
        if !in_exclude || line.is_empty() {
            continue;
        }
        let (include, rule) = line
            .strip_prefix('!')
            .map_or((false, line), |rule| (true, rule));
        let rule = rule.strip_prefix("test262/").unwrap_or(rule);
        let matches = if rule.ends_with('/') {
            path.starts_with(rule)
        } else {
            path == rule
        };
        if matches {
            excluded = !include;
        }
    }
    excluded
}

/// Checks whether `path` is a known failure in the pinned QuickJS reference
/// runner's generated error file. Each line starts with a test path followed
/// by a source location and diagnostic; strict/sloppy duplicates collapse to
/// the same accountable path.
pub fn quickjs_errorfile_contains_path(errorfile: &str, path: &str) -> bool {
    errorfile.lines().any(|line| {
        let line = line.trim();
        let Some(js_end) = line.find(".js:") else {
            return false;
        };
        line[..js_end + 3]
            .strip_prefix("test262/")
            .is_some_and(|known| known == path)
    })
}

/// Produces a side-effect-free observation program.
///
/// Arbitrary objects are deliberately opaque: even reflection (`ownKeys`,
/// `getPrototypeOf`, `instanceof`, or `toString`) can execute user Proxy traps.
/// Suites needing object-graph comparison must make an explicit snapshot in
/// the fixture while its intended effects can be recorded in the event log.
pub const fn canonical_observer_prelude() -> &'static str {
    r#"
(() => {
  // Capture every observer before evaluating untrusted code.  In particular,
  // the expression may poison any global or intrinsic property.
  const $apply = Reflect.apply;
  const $numberIsNaN = Number.isNaN;
  const $objectIs = Object.is;
  const $symbolKeyFor = Symbol.keyFor;
  const $string = String;
  const $jsonStringify = JSON.stringify;
  const primitive = value => {
    if (value === undefined) return {type:'undefined'};
    if (typeof value === 'number') {
      if ($apply($numberIsNaN, undefined, [value])) return {type:'number', value:'NaN'};
      if ($apply($objectIs, undefined, [value, -0])) return {type:'number', value:'-0'};
      if (value === Infinity) return {type:'number', value:'+Infinity'};
      if (value === -Infinity) return {type:'number', value:'-Infinity'};
    }
    if (typeof value === 'bigint') return {type:'bigint', value:$apply($string, undefined, [value])};
    if (typeof value === 'symbol') return {type:'symbol', global:$apply($symbolKeyFor, undefined, [value]) ?? null};
    return {type:typeof value, value};
  };
  const observe = value =>
    ((typeof value !== 'object' || value === null) && typeof value !== 'function')
      ? primitive(value) : {type:typeof value,opaque:true};
  const observer = thunk => {
    try { return $apply($jsonStringify, undefined, [{ok:true, coverage:'primitive-only', value:observe(thunk())}]); }
    catch (error) { return $apply($jsonStringify, undefined, [{ok:false, coverage:'primitive-only', exception:observe(error)}]); }
  };
  Object.defineProperty(globalThis, '__rquickjsTrustedObserve', {value:observer,writable:false,configurable:false,enumerable:false});
})()
"#
}

/// Calls the trusted observer. The caller must evaluate
/// [`canonical_observer_prelude`] before any fixture, harness, or test source.
pub fn canonical_observation_source(expression: &str) -> String {
    canonical_observation_call_source(expression)
}

pub fn canonical_observation_call_source(expression: &str) -> String {
    format!("__rquickjsTrustedObserve(()=>({expression}))")
}

/// Observes a fixture-declared, Proxy-free plain data graph.
///
/// The caller must own the expression and guarantee it contains only plain
/// objects/arrays and primitives. This explicit capability is intentionally
/// separate from [`canonical_observation_source`], which is safe for arbitrary
/// JavaScript values but treats objects as opaque.
pub fn canonical_plain_data_observer_prelude() -> String {
    format!(
        r#"{}(() => {{
 const $apply=Reflect.apply,$ownKeys=Reflect.ownKeys,$getDescriptors=Object.getOwnPropertyDescriptors,$arrayIsArray=Array.isArray,$numberIsNaN=Number.isNaN,$objectIs=Object.is,$symbolKeyFor=Symbol.keyFor,$string=String,$jsonStringify=JSON.stringify,$JSON=JSON;
 const seen=new Map(); let nextId=1;
 const primitive=value=>{{
  if(value===undefined)return{{type:'undefined'}};
  if(typeof value==='number'){{if($apply($numberIsNaN,undefined,[value]))return{{type:'number',value:'NaN'}};if($apply($objectIs,undefined,[value,-0]))return{{type:'number',value:'-0'}};}}
  if(typeof value==='bigint')return{{type:'bigint',value:$apply($string,undefined,[value])}};
  if(typeof value==='symbol')return{{type:'symbol',global:$apply($symbolKeyFor,undefined,[value])??null}};
  return{{type:typeof value,value}};
 }};
 const observe=value=>{{
  if((typeof value!=='object'||value===null)&&typeof value!=='function')return primitive(value);
  if(seen.has(value))return{{ref:seen.get(value)}};
  const id=nextId++;seen.set(value,id);
  const descriptors=$apply($getDescriptors,undefined,[value]);
  const properties=$apply($ownKeys,undefined,[descriptors]).map(key=>{{const d=descriptors[key];return{{key:observe(key),enumerable:d.enumerable,configurable:d.configurable,writable:'writable'in d?d.writable:null,value:'value'in d?observe(d.value):null,get:null,set:null}}}});
  const array=$apply($arrayIsArray,undefined,[value]);
  return{{id,tag:array?'[object Array]':'[object Object]',prototype:array?'Array':'Object',properties}};
 }};
 const observer=thunk=>$apply($jsonStringify,undefined,[{{ok:true,value:observe(thunk())}}]);
 Object.defineProperty(globalThis,'__rquickjsTrustedPlainObserve',{{value:observer,writable:false,configurable:false,enumerable:false}});
}})()"#,
        ""
    )
}

pub fn canonical_plain_data_observation_source(expression: &str) -> String {
    format!("__rquickjsTrustedPlainObserve(()=>({expression}))")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuredProgram {
    seed: u64,
    fuel: u16,
    source: String,
}

impl StructuredProgram {
    pub fn generate(seed: u64, fuel: u16) -> Self {
        let fuel = fuel.clamp(1, 256);
        let mut state = seed;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state
        };
        let bound = (next() % u64::from(fuel.max(2))) + 1;
        let a = next() % 97;
        let b = next() % 31;
        let source = format!(
            r#"(()=>{{
const events=[]; const target={{x:{a},get y(){{events.push('get');return this.x}}}};
const proxy=new Proxy(target,{{get(o,k,r){{events.push('proxy:'+String(k));return Reflect.get(o,k,r)}}}});
let total=0; for(let i=0;i<{bound};i++){{ try{{total=(total+proxy.y+i+{b})|0}}catch(e){{events.push(e.name)}} finally{{total|=0}} }}
const closure=x=>()=>x+total; const array=[target,,proxy];
return total+'|'+closure(1)()+'|'+(array[0]===target)+'|'+events.join(',');
}})()"#
        );
        Self { seed, fuel, source }
    }
    pub const fn seed(&self) -> u64 {
        self.seed
    }
    pub const fn fuel(&self) -> u16 {
        self.fuel
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn shrink(&self) -> Self {
        Self::generate(self.seed, (self.fuel / 2).max(1))
    }
}
