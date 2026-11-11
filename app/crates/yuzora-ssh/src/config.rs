//! `~/.ssh/config`: parsing (Include, Host and Match), host listing and resolving.
//!
//! A subset of `ssh_config(5)`. The file is parsed once into a flat list of
//! items (Include bodies are spliced in place); [`SshConfig::resolve`] then walks
//! that list in order, so `Match` sees the values obtained before it, as ssh does.

use std::collections::HashSet;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Files nested deeper than this are not read (OpenSSH allows 16 levels of Include).
const MAX_INCLUDE_DEPTH: usize = 16;

/// What `~`, `%u` and relative Include paths depend on; always passed in so nothing reads the real environment by itself.
#[derive(Clone, Debug)]
pub struct ConfigContext {
    /// The user's home directory (`~`, `%d`, `<home>/.ssh`).
    pub home: PathBuf,
    /// The local user name (`%u`, `Match localuser`, the default remote user).
    pub local_user: String,
}

impl ConfigContext {
    /// std::env::home_dir() and $USER / $USERNAME ("" when unknown).
    pub fn system() -> Self {
        let home = std::env::home_dir().unwrap_or_default();
        let local_user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
        Self { home, local_user }
    }

    /// <home>/.ssh/config
    pub fn user_config(&self) -> PathBuf {
        self.home.join(".ssh").join("config")
    }
}

/// A parsed `ssh_config` (the user's file plus everything it includes).
#[derive(Clone, Debug, Default)]
pub struct SshConfig {
    items: Vec<Item>,
    hosts: Vec<ConfigHost>,
    warnings: Vec<String>,
}

/// A concrete `Host` alias and where it was declared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConfigHost {
    /// The alias as written on the `Host` line.
    pub alias: String,
    /// The file holding the `Host` line.
    pub file: PathBuf,
    /// 1-based line number of the `Host` line.
    pub line: usize,
}

/// `StrictHostKeyChecking`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StrictHostKeyChecking {
    /// Prompt for unknown keys (the default).
    Ask,
    /// Refuse unknown keys.
    Yes,
    /// Accept anything.
    No,
    /// Accept and record new hosts, refuse changed keys.
    AcceptNew,
}

/// One hop of a `ProxyJump` chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JumpHost {
    /// Remote user, when the destination names one.
    pub user: Option<String>,
    /// Host name or address (IPv6 without brackets).
    pub host: String,
    /// Port, when the destination names one.
    pub port: Option<u16>,
}

/// Everything `ssh <host>` would use, with OpenSSH's defaults filled in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResolvedHost {
    /// The name as asked.
    pub alias: String,
    /// HostName (`%h` -> alias, `%%` -> `%`) or the alias.
    pub hostname: String,
    /// User or the local user.
    pub user: String,
    /// Port or 22.
    pub port: u16,
    /// Every IdentityFile, in order, expanded; empty if none.
    pub identity_files: Vec<PathBuf>,
    /// IdentitiesOnly.
    pub identities_only: bool,
    /// IdentityAgent, expanded; "none" stays "none".
    pub identity_agent: Option<String>,
    /// ProxyJump hops; empty when unset or "none".
    pub proxy_jump: Vec<JumpHost>,
    /// ProxyCommand with tokens expanded; None when unset or "none".
    pub proxy_command: Option<String>,
    /// UserKnownHostsFile (default `<home>/.ssh/known_hosts` and `known_hosts2`).
    pub user_known_hosts_files: Vec<PathBuf>,
    /// GlobalKnownHostsFile (default `/etc/ssh/ssh_known_hosts` and `ssh_known_hosts2`).
    pub global_known_hosts_files: Vec<PathBuf>,
    /// StrictHostKeyChecking (default ask).
    pub strict_host_key_checking: StrictHostKeyChecking,
    /// HostKeyAlias.
    pub host_key_alias: Option<String>,
    /// ConnectTimeout in seconds.
    pub connect_timeout: Option<u64>,
    /// ServerAliveInterval in seconds.
    pub server_alive_interval: Option<u64>,
    /// ServerAliveCountMax.
    pub server_alive_count_max: Option<u32>,
    /// PreferredAuthentications, lowercased; empty means the default order.
    pub preferred_authentications: Vec<String>,
    /// PasswordAuthentication (default true).
    pub password_authentication: bool,
    /// PubkeyAuthentication (default true).
    pub pubkey_authentication: bool,
    /// KbdInteractiveAuthentication / ChallengeResponseAuthentication (default true).
    pub kbd_interactive_authentication: bool,
    /// ForwardAgent (a socket path or $VAR counts as true).
    pub forward_agent: bool,
}

impl SshConfig {
    /// Reads the file and every Include; a missing or unreadable file is an empty config.
    pub fn load(path: &Path, ctx: &ConfigContext) -> SshConfig {
        match fs::read(path) {
            Ok(bytes) => Self::parse_str(&String::from_utf8_lossy(&bytes), path, ctx),
            Err(_) => SshConfig::default(),
        }
    }

    /// Same from text; `origin` only labels lines. Relative Include paths are under <home>/.ssh.
    pub fn parse_str(text: &str, origin: &Path, ctx: &ConfigContext) -> SshConfig {
        let mut parser = Parser { ctx, cfg: SshConfig::default(), seen: HashSet::new(), chain: vec![canon(origin)] };
        parser.parse(text, origin, 0);
        parser.cfg
    }

    /// Named hosts: Host-line patterns without * ? or !, first appearance wins, file order.
    pub fn hosts(&self) -> Vec<ConfigHost> {
        self.hosts.clone()
    }

    /// What `ssh <host>` would use (first obtained value wins).
    pub fn resolve(&self, host: &str, ctx: &ConfigContext) -> ResolvedHost {
        let mut acc = Acc::default();
        // `active`: do the options being read apply to this host. `never`: inside an
        // Include opened from an inactive block, where even Host and Match cannot match.
        let (mut active, mut never) = (true, false);
        let mut saved: Vec<(bool, bool)> = Vec::new();
        for item in &self.items {
            match item {
                Item::Host(patterns) => active = !never && list_matches(patterns.iter().map(String::as_str), host, true),
                Item::Match(criteria) => active = !never && criteria.iter().all(|c| c.holds(host, ctx, &acc)),
                Item::Enter => {
                    saved.push((active, never));
                    never = never || !active;
                }
                Item::Leave => {
                    if let Some(state) = saved.pop() {
                        (active, never) = state;
                    }
                }
                Item::Opt(opt) if active => acc.apply(opt, host),
                Item::Opt(_) => {}
            }
        }
        acc.finish(host, ctx)
    }

    /// Things skipped: unsupported Match criteria, Include depth, bad ports...
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// "[user@]host[:port]", "user@[v6]:port", "ssh://[user@]host[:port]". None for empty/invalid.
/// A host or user name OpenSSH would take from outside its config: no shell
/// metacharacters, whitespace or control characters, and no leading `-`.
/// `%h`/`%r` go into a ProxyCommand's shell line and the destination into an
/// `ssh` argv, so anything else is refused.
pub fn safe_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && !s.chars().any(|c| c.is_whitespace() || c.is_control() || "'`\"$\\;&<>|(){}".contains(c))
}

pub fn parse_destination(s: &str) -> Option<JumpHost> {
    let s = s.trim();
    let s = s.strip_prefix("ssh://").map(|r| r.trim_end_matches('/')).unwrap_or(s);
    let (user, hostport) = match s.rfind('@') {
        Some(i) => {
            let user = &s[..i];
            if user.is_empty() || user.contains(char::is_whitespace) {
                return None;
            }
            (Some(user.to_string()), &s[i + 1..])
        }
        None => (None, s),
    };
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = if after.is_empty() { None } else { Some(after.strip_prefix(':')?) };
        (host, port)
    } else if hostport.matches(':').count() == 1 {
        let (host, port) = hostport.split_once(':')?;
        (host, Some(port))
    } else {
        // No colon, or several: a bare IPv6 address has no port.
        (hostport, None)
    };
    let port = match port {
        Some(p) => Some(parse_port(p)?),
        None => None,
    };
    if host.is_empty() || host.contains(|c: char| c.is_whitespace() || c == '/' || c == '@') {
        return None;
    }
    Some(JumpHost { user, host: host.to_string(), port })
}

/// `*` and `?` wildcards, case-sensitive; shared with the known_hosts matcher.
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    glob(&p, &t, false)
}

#[derive(Clone, Debug)]
enum Item {
    Host(Vec<String>),
    Match(Vec<Criterion>),
    /// Start / end of an Include body.
    Enter,
    Leave,
    Opt(Opt),
}

#[derive(Clone, Debug)]
struct Opt {
    /// Lowercased keyword.
    key: String,
    /// Arguments with quotes removed.
    args: Vec<String>,
    /// The raw remainder of the line (ProxyCommand keeps its quoting).
    rest: String,
}

#[derive(Clone, Debug)]
struct Criterion {
    kind: Kind,
    negate: bool,
}

#[derive(Clone, Debug)]
enum Kind {
    /// `all`, `canonical`, `final`.
    Always,
    /// Unsupported or malformed: never true, even negated.
    Never,
    Host(String),
    OriginalHost(String),
    User(String),
    LocalUser(String),
}

impl Criterion {
    fn holds(&self, alias: &str, ctx: &ConfigContext, acc: &Acc) -> bool {
        let truth = match &self.kind {
            Kind::Never => return false,
            Kind::Always => true,
            Kind::Host(list) => list_matches(list.split(',').filter(|p| !p.is_empty()), acc.hostname.as_deref().unwrap_or(alias), true),
            Kind::OriginalHost(list) => list_matches(list.split(',').filter(|p| !p.is_empty()), alias, true),
            Kind::User(list) => list_matches(list.split(',').filter(|p| !p.is_empty()), acc.user.as_deref().unwrap_or(&ctx.local_user), false),
            Kind::LocalUser(list) => list_matches(list.split(',').filter(|p| !p.is_empty()), &ctx.local_user, false),
        };
        truth != self.negate
    }
}

struct Parser<'a> {
    ctx: &'a ConfigContext,
    cfg: SshConfig,
    seen: HashSet<String>,
    /// Files currently being read, to stop Include cycles.
    chain: Vec<PathBuf>,
}

impl Parser<'_> {
    fn warn(&mut self, file: &Path, line: usize, msg: impl Display) {
        self.cfg.warnings.push(format!("{}:{}: {}", file.display(), line, msg));
    }

    fn parse(&mut self, text: &str, origin: &Path, depth: usize) {
        for (i, raw) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            self.line(raw, origin, i + 1, depth);
        }
    }

    fn line(&mut self, raw: &str, origin: &Path, no: usize, depth: usize) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            return;
        }
        let (keyword, rest) = split_keyword(line);
        let key = keyword.to_ascii_lowercase();
        let args = split_args(rest);
        match key.as_str() {
            "host" => {
                for p in &args {
                    if !p.contains(['*', '?', '!']) && self.seen.insert(p.clone()) {
                        self.cfg.hosts.push(ConfigHost { alias: p.clone(), file: origin.to_path_buf(), line: no });
                    }
                }
                self.cfg.items.push(Item::Host(args));
            }
            "match" => {
                let criteria = self.criteria(&args, origin, no);
                self.cfg.items.push(Item::Match(criteria));
            }
            "include" => self.include(&args, origin, no, depth),
            _ => {
                self.check_value(&key, &args, origin, no);
                self.cfg.items.push(Item::Opt(Opt { key, args, rest: rest.to_string() }));
            }
        }
    }

    fn criteria(&mut self, args: &[String], origin: &Path, no: usize) -> Vec<Criterion> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let (negate, name) = match args[i].strip_prefix('!') {
                Some(name) => (true, name),
                None => (false, args[i].as_str()),
            };
            i += 1;
            let name = name.to_ascii_lowercase();
            let kind = match name.as_str() {
                "all" | "canonical" | "final" => Kind::Always,
                "host" | "originalhost" | "user" | "localuser" => match args.get(i) {
                    Some(list) => {
                        i += 1;
                        let list = list.clone();
                        match name.as_str() {
                            "host" => Kind::Host(list),
                            "originalhost" => Kind::OriginalHost(list),
                            "user" => Kind::User(list),
                            _ => Kind::LocalUser(list),
                        }
                    }
                    None => {
                        self.warn(origin, no, format!("Match {name} needs an argument"));
                        Kind::Never
                    }
                },
                "exec" | "localnetwork" | "tagged" | "command" | "version" | "sessiontype" => {
                    i += 1;
                    self.warn(origin, no, format!("Match {name} is not supported; the block never matches"));
                    Kind::Never
                }
                _ => {
                    self.warn(origin, no, format!("unknown Match criterion {name:?}; the block never matches"));
                    Kind::Never
                }
            };
            out.push(Criterion { kind, negate });
        }
        if out.is_empty() {
            self.warn(origin, no, "Match without criteria never matches");
            out.push(Criterion { kind: Kind::Never, negate: false });
        }
        out
    }

    /// Warns about values the resolver will ignore (it skips them silently).
    fn check_value(&mut self, key: &str, args: &[String], origin: &Path, no: usize) {
        let first = args.first().map(String::as_str).unwrap_or("");
        match key {
            "port" if parse_port(first).is_none() => self.warn(origin, no, format!("invalid Port {first:?} ignored")),
            "connecttimeout" | "serveraliveinterval" if parse_time(first).is_none() => {
                self.warn(origin, no, format!("invalid {key} {first:?} ignored"));
            }
            "serveralivecountmax" if first.parse::<u32>().is_err() => {
                self.warn(origin, no, format!("invalid ServerAliveCountMax {first:?} ignored"));
            }
            "proxyjump" if !first.eq_ignore_ascii_case("none") => {
                for dest in first.split(',') {
                    if parse_destination(dest).is_none() {
                        self.warn(origin, no, format!("invalid ProxyJump destination {dest:?} ignored"));
                    }
                }
            }
            _ => {}
        }
    }

    fn include(&mut self, args: &[String], origin: &Path, no: usize, depth: usize) {
        if depth >= MAX_INCLUDE_DEPTH {
            self.warn(origin, no, format!("Include nested more than {MAX_INCLUDE_DEPTH} levels deep, skipped"));
            return;
        }
        self.cfg.items.push(Item::Enter);
        for arg in args {
            for path in include_paths(arg, self.ctx) {
                let Ok(bytes) = fs::read(&path) else { continue };
                let canonical = canon(&path);
                if self.chain.contains(&canonical) {
                    self.warn(origin, no, format!("Include cycle at {}, skipped", path.display()));
                    continue;
                }
                self.chain.push(canonical);
                self.parse(&String::from_utf8_lossy(&bytes), &path, depth + 1);
                self.chain.pop();
            }
        }
        self.cfg.items.push(Item::Leave);
    }
}

/// Values obtained so far while resolving one host (first one wins).
#[derive(Default)]
struct Acc {
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_files: Vec<String>,
    identities_only: Option<bool>,
    identity_agent: Option<String>,
    proxy: Option<Proxy>,
    user_known_hosts: Option<Vec<String>>,
    global_known_hosts: Option<Vec<String>>,
    strict: Option<StrictHostKeyChecking>,
    host_key_alias: Option<String>,
    connect_timeout: Option<u64>,
    server_alive_interval: Option<u64>,
    server_alive_count_max: Option<u32>,
    preferred: Option<Vec<String>>,
    password: Option<bool>,
    pubkey: Option<bool>,
    kbd: Option<bool>,
    forward_agent: Option<bool>,
}

/// ProxyJump and ProxyCommand compete: whichever comes first wins, and `none` counts.
enum Proxy {
    None,
    Jump(Vec<JumpHost>),
    Command(String),
}

fn set<T>(slot: &mut Option<T>, value: Option<T>) {
    if slot.is_none() {
        *slot = value;
    }
}

impl Acc {
    fn apply(&mut self, opt: &Opt, alias: &str) {
        let first = opt.args.first().map(String::as_str);
        let text = || first.map(String::from);
        match opt.key.as_str() {
            "hostname" => set(&mut self.hostname, first.map(|v| expand_hostname(v, alias))),
            "user" => set(&mut self.user, text()),
            "port" => set(&mut self.port, first.and_then(parse_port)),
            "identityfile" => self.identity_files.extend(text()),
            "identitiesonly" => set(&mut self.identities_only, first.and_then(parse_bool)),
            "identityagent" => set(&mut self.identity_agent, text()),
            "proxyjump" => set(
                &mut self.proxy,
                first.map(|v| {
                    if v.eq_ignore_ascii_case("none") {
                        Proxy::None
                    } else {
                        Proxy::Jump(v.split(',').filter_map(parse_destination).collect())
                    }
                }),
            ),
            "proxycommand" if !opt.rest.is_empty() => set(
                &mut self.proxy,
                Some(if opt.rest.eq_ignore_ascii_case("none") { Proxy::None } else { Proxy::Command(opt.rest.clone()) }),
            ),
            "userknownhostsfile" => set(&mut self.user_known_hosts, file_list(&opt.args)),
            "globalknownhostsfile" => set(&mut self.global_known_hosts, file_list(&opt.args)),
            "stricthostkeychecking" => set(&mut self.strict, first.and_then(parse_strict)),
            "hostkeyalias" => set(&mut self.host_key_alias, text()),
            "connecttimeout" => set(&mut self.connect_timeout, first.and_then(parse_time)),
            "serveraliveinterval" => set(&mut self.server_alive_interval, first.and_then(parse_time)),
            "serveralivecountmax" => set(&mut self.server_alive_count_max, first.and_then(|v| v.parse().ok())),
            "preferredauthentications" if !opt.args.is_empty() => set(
                &mut self.preferred,
                Some(opt.args.join(",").split(',').map(|m| m.trim().to_ascii_lowercase()).filter(|m| !m.is_empty()).collect()),
            ),
            "passwordauthentication" => set(&mut self.password, first.and_then(parse_bool)),
            "pubkeyauthentication" => set(&mut self.pubkey, first.and_then(parse_bool)),
            "kbdinteractiveauthentication" | "challengeresponseauthentication" => set(&mut self.kbd, first.and_then(parse_bool)),
            "forwardagent" => set(&mut self.forward_agent, first.map(|v| parse_bool(v).unwrap_or(true))),
            _ => {}
        }
    }

    fn finish(self, alias: &str, ctx: &ConfigContext) -> ResolvedHost {
        let hostname = self.hostname.unwrap_or_else(|| alias.to_string());
        let user = self.user.unwrap_or_else(|| ctx.local_user.clone());
        let port = self.port.unwrap_or(22);
        let tokens = Tokens { home: &ctx.home, hostname: &hostname, alias, port, user: &user, local_user: &ctx.local_user };
        let paths = |list: &[String]| list.iter().map(|s| PathBuf::from(tokens.expand(s))).collect::<Vec<_>>();
        let (proxy_jump, proxy_command) = match &self.proxy {
            Some(Proxy::Jump(hops)) => (hops.clone(), None),
            Some(Proxy::Command(cmd)) => (Vec::new(), Some(tokens.expand(cmd))),
            Some(Proxy::None) | None => (Vec::new(), None),
        };
        ResolvedHost {
            alias: alias.to_string(),
            identity_files: paths(&self.identity_files),
            identities_only: self.identities_only.unwrap_or(false),
            identity_agent: self.identity_agent.as_deref().map(|v| tokens.expand(v)),
            proxy_jump,
            proxy_command,
            user_known_hosts_files: match &self.user_known_hosts {
                Some(list) => paths(list),
                None => vec![ctx.home.join(".ssh").join("known_hosts"), ctx.home.join(".ssh").join("known_hosts2")],
            },
            global_known_hosts_files: match &self.global_known_hosts {
                Some(list) => paths(list),
                None => vec![PathBuf::from("/etc/ssh/ssh_known_hosts"), PathBuf::from("/etc/ssh/ssh_known_hosts2")],
            },
            strict_host_key_checking: self.strict.unwrap_or(StrictHostKeyChecking::Ask),
            host_key_alias: self.host_key_alias,
            connect_timeout: self.connect_timeout,
            server_alive_interval: self.server_alive_interval,
            server_alive_count_max: self.server_alive_count_max,
            preferred_authentications: self.preferred.unwrap_or_default(),
            password_authentication: self.password.unwrap_or(true),
            pubkey_authentication: self.pubkey.unwrap_or(true),
            kbd_interactive_authentication: self.kbd.unwrap_or(true),
            forward_agent: self.forward_agent.unwrap_or(false),
            hostname,
            user,
            port,
        }
    }
}

/// The values behind `%d %h %n %p %r %u`.
struct Tokens<'a> {
    home: &'a Path,
    hostname: &'a str,
    alias: &'a str,
    port: u16,
    user: &'a str,
    local_user: &'a str,
}

impl Tokens<'_> {
    /// A leading `~` plus `%%` and the tokens; unknown tokens are kept as written.
    fn expand(&self, s: &str) -> String {
        let (mut out, tail) = match s.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => (self.home.to_string_lossy().into_owned(), rest),
            _ => (String::new(), s),
        };
        let mut chars = tail.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => out.push('%'),
                Some('d') => out.push_str(&self.home.to_string_lossy()),
                Some('h') => out.push_str(self.hostname),
                Some('n') => out.push_str(self.alias),
                Some('p') => out.push_str(&self.port.to_string()),
                Some('r') => out.push_str(self.user),
                Some('u') => out.push_str(self.local_user),
                Some(other) => {
                    out.push('%');
                    out.push(other);
                }
                None => out.push('%'),
            }
        }
        out
    }
}

/// HostName only knows `%h` (the alias) and `%%`.
fn expand_hostname(s: &str, alias: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('h') => out.push_str(alias),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

fn canon(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `keyword value`, `keyword=value` and `keyword = value`; returns (keyword, value text).
fn split_keyword(line: &str) -> (&str, &str) {
    let end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let (key, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    (key, rest)
}

/// Splits on whitespace; `"..."` groups; `\\` `\"` `\'` escape; an unquoted `#` starts a comment.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut quoted = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let tok = cur.get_or_insert_with(String::new);
                match chars.peek() {
                    Some(&n) if matches!(n, '"' | '\\' | '\'') => {
                        tok.push(n);
                        chars.next();
                    }
                    _ => tok.push('\\'),
                }
            }
            '"' => {
                quoted = !quoted;
                cur.get_or_insert_with(String::new);
            }
            c if c.is_whitespace() && !quoted => out.extend(cur.take()),
            '#' if !quoted && cur.is_none() => break,
            c => cur.get_or_insert_with(String::new).push(c),
        }
    }
    out.extend(cur);
    out
}

/// A file list option: `none` means no files.
fn file_list(args: &[String]) -> Option<Vec<String>> {
    match args {
        [] => None,
        [one] if one.eq_ignore_ascii_case("none") => Some(Vec::new()),
        _ => Some(args.to_vec()),
    }
}

fn parse_port(s: &str) -> Option<u16> {
    s.parse::<u16>().ok().filter(|p| *p != 0)
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        _ => None,
    }
}

fn parse_strict(s: &str) -> Option<StrictHostKeyChecking> {
    match s.to_ascii_lowercase().as_str() {
        "yes" | "true" => Some(StrictHostKeyChecking::Yes),
        "no" | "false" | "off" => Some(StrictHostKeyChecking::No),
        "ask" => Some(StrictHostKeyChecking::Ask),
        "accept-new" => Some(StrictHostKeyChecking::AcceptNew),
        _ => None,
    }
}

/// OpenSSH time: seconds, or numbers with s/m/h/d/w suffixes (`1m30s`).
fn parse_time(s: &str) -> Option<u64> {
    let (mut total, mut digits, mut any) = (0u64, String::new(), false);
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let unit: u64 = match c.to_ascii_lowercase() {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return None,
        };
        total = total.checked_add(digits.parse::<u64>().ok()?.checked_mul(unit)?)?;
        digits.clear();
        any = true;
    }
    if !digits.is_empty() {
        total = total.checked_add(digits.parse::<u64>().ok()?)?;
        any = true;
    }
    any.then_some(total)
}

/// OpenSSH `match_pattern_list`: some pattern matches and no `!pattern` does.
fn list_matches<'a>(patterns: impl IntoIterator<Item = &'a str>, text: &str, fold_case: bool) -> bool {
    let text = if fold_case { text.to_lowercase() } else { text.to_string() };
    let mut hit = false;
    for p in patterns {
        let (negated, p) = match p.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, p),
        };
        let found = if fold_case { wildcard_match(&p.to_lowercase(), &text) } else { wildcard_match(p, &text) };
        if found {
            if negated {
                return false;
            }
            hit = true;
        }
    }
    hit
}

/// Backtracking matcher for `*` and `?`; with `classes` also `[a-z]` / `[!x]`.
fn glob(pat: &[char], text: &[char], classes: bool) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        let step = match pat.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some('?') => Some(1),
            Some('[') if classes => match char_class(&pat[p..], text[t]) {
                Some((true, len)) => Some(len),
                Some((false, _)) => None,
                None => (text[t] == '[').then_some(1),
            },
            Some(&c) => (c == text[t]).then_some(1),
            None => None,
        };
        match step {
            Some(len) => {
                p += len;
                t += 1;
            }
            None => match star {
                Some((sp, st)) => {
                    p = sp + 1;
                    t = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    while pat.get(p) == Some(&'*') {
        p += 1;
    }
    p == pat.len()
}

/// `pat` starts with `[`: (does `c` match, length of the class); None when it never closes.
fn char_class(pat: &[char], c: char) -> Option<(bool, usize)> {
    let mut i = 1;
    let negated = matches!(pat.get(i), Some('!' | '^'));
    if negated {
        i += 1;
    }
    let mut hit = false;
    let mut first = true;
    loop {
        let &lo = pat.get(i)?;
        if lo == ']' && !first {
            return Some((hit != negated, i + 1));
        }
        first = false;
        if pat.get(i + 1) == Some(&'-') && pat.get(i + 2).is_some_and(|&hi| hi != ']') {
            let hi = pat[i + 2];
            hit |= lo <= c && c <= hi;
            i += 3;
        } else {
            hit |= lo == c;
            i += 1;
        }
    }
}

fn has_glob(segment: &str) -> bool {
    segment.contains(['*', '?', '['])
}

/// One path segment against a glob; like glob(3), a leading `.` must be matched explicitly.
fn segment_matches(pattern: &str, name: &str) -> bool {
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    glob(&p, &n, true)
}

/// Files an `Include` argument names: `~/`, paths under `<home>/.ssh`, globs sorted.
fn include_paths(arg: &str, ctx: &ConfigContext) -> Vec<PathBuf> {
    let (base, rel) = if arg == "~" {
        (ctx.home.clone(), "")
    } else if let Some(rest) = arg.strip_prefix("~/") {
        (ctx.home.clone(), rest)
    } else if Path::new(arg).is_absolute() {
        (PathBuf::new(), arg)
    } else {
        (ctx.home.join(".ssh"), arg)
    };
    if !rel.split(['/', '\\']).any(has_glob) {
        return vec![base.join(rel)];
    }
    let mut dirs = vec![base];
    for component in Path::new(rel).components() {
        let name = component.as_os_str().to_string_lossy();
        if !has_glob(&name) {
            dirs = dirs.into_iter().map(|d| d.join(component.as_os_str())).collect();
            continue;
        }
        let mut next = Vec::new();
        for dir in &dirs {
            let Ok(entries) = fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                if segment_matches(&name, &entry.file_name().to_string_lossy()) {
                    next.push(dir.join(entry.file_name()));
                }
            }
        }
        dirs = next;
    }
    dirs.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("yuzora-ssh-test-{}-config-{}", std::process::id(), name));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        /// Writes `<dir>/<rel>` (parents created) and returns its path.
        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }

        fn ctx(&self) -> ConfigContext {
            ConfigContext { home: self.0.clone(), local_user: "me".into() }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fake_ctx() -> ConfigContext {
        ConfigContext { home: PathBuf::from("/home/me"), local_user: "me".into() }
    }

    fn parse(text: &str) -> SshConfig {
        SshConfig::parse_str(text, Path::new("test.conf"), &fake_ctx())
    }

    fn resolve(text: &str, host: &str) -> ResolvedHost {
        parse(text).resolve(host, &fake_ctx())
    }

    fn dest(user: Option<&str>, host: &str, port: Option<u16>) -> JumpHost {
        JumpHost { user: user.map(String::from), host: host.into(), port }
    }

    #[test]
    fn defaults_when_nothing_is_configured() {
        let r = resolve("", "box");
        assert_eq!(r.alias, "box");
        assert_eq!(r.hostname, "box");
        assert_eq!(r.user, "me");
        assert_eq!(r.port, 22);
        assert!(r.identity_files.is_empty());
        assert!(!r.identities_only);
        assert_eq!(r.identity_agent, None);
        assert!(r.proxy_jump.is_empty());
        assert_eq!(r.proxy_command, None);
        assert_eq!(r.user_known_hosts_files, vec![PathBuf::from("/home/me/.ssh/known_hosts"), PathBuf::from("/home/me/.ssh/known_hosts2")]);
        assert_eq!(r.global_known_hosts_files, vec![PathBuf::from("/etc/ssh/ssh_known_hosts"), PathBuf::from("/etc/ssh/ssh_known_hosts2")]);
        assert_eq!(r.strict_host_key_checking, StrictHostKeyChecking::Ask);
        assert_eq!(r.host_key_alias, None);
        assert_eq!((r.connect_timeout, r.server_alive_interval, r.server_alive_count_max), (None, None, None));
        assert!(r.preferred_authentications.is_empty());
        assert!(r.password_authentication && r.pubkey_authentication && r.kbd_interactive_authentication);
        assert!(!r.forward_agent);
    }

    #[test]
    fn first_obtained_value_wins() {
        let text = "Host box\n  User one\n  Port 2200\nHost *\n  User two\n  Port 2300\n  HostName real.example\n  HostName later.example\n";
        let r = resolve(text, "box");
        assert_eq!((r.user.as_str(), r.port, r.hostname.as_str()), ("one", 2200, "real.example"));
        let other = resolve(text, "elsewhere");
        assert_eq!((other.user.as_str(), other.port), ("two", 2300));
    }

    #[test]
    fn keywords_ignore_case_and_accept_equals_and_quotes() {
        let text = "# comment\n\n  hostname=a.example\nPORT = 2222\nUser   \"dev user\"\nIdentityFile \"/keys/my key\"  # trailing\nIdentityFile=/keys/plain\n";
        let r = resolve(text, "x");
        assert_eq!(r.hostname, "a.example");
        assert_eq!(r.port, 2222);
        assert_eq!(r.user, "dev user");
        assert_eq!(r.identity_files, vec![PathBuf::from("/keys/my key"), PathBuf::from("/keys/plain")]);
    }

    #[test]
    fn host_patterns_wildcards_and_negation() {
        let text = "Host *.example.com !bad.example.com\n  User wild\nHost web? db\n  Port 2022\n";
        let c = parse(text);
        assert_eq!(c.resolve("a.example.com", &fake_ctx()).user, "wild");
        assert_eq!(c.resolve("A.Example.COM", &fake_ctx()).user, "wild");
        assert_eq!(c.resolve("bad.example.com", &fake_ctx()).user, "me");
        assert_eq!(c.resolve("example.org", &fake_ctx()).user, "me");
        assert_eq!(c.resolve("web1", &fake_ctx()).port, 2022);
        assert_eq!(c.resolve("db", &fake_ctx()).port, 2022);
        assert_eq!(c.resolve("web12", &fake_ctx()).port, 22);
        // Only negations never match.
        assert_eq!(parse("Host !x\n User n\n").resolve("y", &fake_ctx()).user, "me");
    }

    #[test]
    fn match_host_sees_the_hostname_obtained_so_far() {
        let text = "Host a\n  HostName real.example\nMatch host real.example\n  User by-hostname\nMatch originalhost a\n  Port 2200\nMatch host a\n  IdentitiesOnly yes\n";
        let r = resolve(text, "a");
        assert_eq!(r.user, "by-hostname");
        assert_eq!(r.port, 2200);
        assert!(!r.identities_only, "Match host compares the HostName, not the alias");
        // Without HostName the alias is the host.
        let r = resolve("Match host b\n  User plain\n", "b");
        assert_eq!(r.user, "plain");
    }

    #[test]
    fn match_before_hostname_uses_the_alias() {
        let text = "Match host a\n  User early\nHost a\n  HostName real.example\n";
        assert_eq!(resolve(text, "a").user, "early");
    }

    #[test]
    fn match_user_localuser_all_and_negation() {
        let text = "Host a\n  User alice\nMatch user alice host a\n  Port 2201\nMatch user bob\n  Port 2202\nMatch localuser me\n  IdentitiesOnly yes\nMatch localuser !me\n  ForwardAgent yes\nMatch !host a\n  HostKeyAlias neg\nMatch host !a,*\n  ConnectTimeout 5\n";
        let a = resolve(text, "a");
        assert_eq!(a.port, 2201);
        assert!(a.identities_only);
        assert!(!a.forward_agent);
        assert_eq!(a.host_key_alias, None);
        assert_eq!(a.connect_timeout, None, "a negated list entry excludes the host");
        let b = resolve(text, "b");
        assert_eq!(b.port, 22);
        assert_eq!(b.host_key_alias.as_deref(), Some("neg"));
        assert_eq!(b.connect_timeout, Some(5));
        // `all` applies to everything, `!all` to nothing; canonical and final hold.
        assert_eq!(resolve("Match all\n  Port 3000\n", "z").port, 3000);
        assert_eq!(resolve("Match !all\n  Port 3000\n", "z").port, 22);
        assert_eq!(resolve("Match final all\n  Port 3001\n", "z").port, 3001);
        assert_eq!(resolve("Match canonical host z\n  Port 3002\n", "z").port, 3002);
    }

    #[test]
    fn match_user_compares_the_user_obtained_so_far() {
        let text = "Match user root\n  Port 1\nHost r\n  User root\nMatch user root\n  Port 2\n";
        assert_eq!(resolve(text, "r").port, 2);
        // No User: the local user is compared.
        assert_eq!(resolve("Match user me\n  Port 9\n", "q").port, 9);
        // User names are case-sensitive.
        assert_eq!(resolve("Match user ME\n  Port 9\n", "q").port, 22);
    }

    #[test]
    fn unsupported_match_criteria_warn_and_never_match() {
        let c = parse("Match exec \"test -f /x\"\n  Port 2000\nMatch !exec true\n  User neg\nMatch host z exec true\n  Port 2001\nMatch final\n  HostKeyAlias fine\n");
        let r = c.resolve("z", &fake_ctx());
        assert_eq!(r.port, 22);
        assert_eq!(r.user, "me");
        assert_eq!(r.host_key_alias.as_deref(), Some("fine"));
        let w = c.warnings();
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w[0].starts_with("test.conf:1:") && w[0].contains("exec"));
        for kw in ["localnetwork", "tagged", "command", "version", "sessiontype"] {
            let c = parse(&format!("Match {kw} x\n  Port 2000\n"));
            assert_eq!(c.resolve("z", &fake_ctx()).port, 22, "{kw}");
            assert_eq!(c.warnings().len(), 1, "{kw}");
        }
        assert_eq!(parse("Match host\n").warnings().len(), 1);
    }

    #[test]
    fn include_resolves_relative_to_dot_ssh_and_tilde() {
        let t = TempDir::new("include-basic");
        t.write(".ssh/rel.conf", "Port 2001\n");
        t.write("tilde.conf", "HostKeyAlias tilde\n");
        let abs = t.write("elsewhere/abs.conf", "ConnectTimeout 7\n");
        let main = t.write(".ssh/config", &format!("Include rel.conf\nInclude ~/tilde.conf nope.conf\nInclude \"{}\"\nInclude /does/not/exist\n", abs.display()));
        let cfg = SshConfig::load(&main, &t.ctx());
        let r = cfg.resolve("anything", &t.ctx());
        assert_eq!((r.port, r.host_key_alias.as_deref(), r.connect_timeout), (2001, Some("tilde"), Some(7)));
        assert!(cfg.warnings().is_empty(), "{:?}", cfg.warnings());
    }

    #[test]
    fn include_glob_is_sorted_and_skips_hidden_files() {
        let t = TempDir::new("include-glob");
        t.write(".ssh/conf.d/20-b.conf", "IdentityFile b\n");
        t.write(".ssh/conf.d/10-a.conf", "IdentityFile a\n");
        t.write(".ssh/conf.d/30-c.txt", "IdentityFile txt\n");
        t.write(".ssh/conf.d/.hidden.conf", "IdentityFile hidden\n");
        t.write(".ssh/dirs/two/ssh.conf", "IdentityFile two\n");
        t.write(".ssh/dirs/one/ssh.conf", "IdentityFile one\n");
        t.write(".ssh/x1.conf", "IdentityFile x1\n");
        t.write(".ssh/x3.conf", "IdentityFile x3\n");
        t.write(".ssh/x2.conf", "IdentityFile x2\n");
        let cfg = SshConfig::parse_str("Include conf.d/*.conf\nInclude dirs/*/ssh.conf\nInclude x[1-2].conf\nInclude x?.conf\n", &t.0.join(".ssh/config"), &t.ctx());
        let names: Vec<_> = cfg.resolve("h", &t.ctx()).identity_files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["a", "b", "one", "two", "x1", "x2", "x1", "x2", "x3"]);
    }

    #[test]
    fn include_inside_a_block_is_conditional() {
        let t = TempDir::new("include-conditional");
        t.write(".ssh/work.conf", "User worker\nHost other\n  User otheruser\n");
        t.write(".ssh/after.conf", "HostKeyAlias after\n");
        let text = "Host work other\n  Include work.conf\n  Port 2200\nHost work\n  Include after.conf\nHost lonely\n  ConnectTimeout 3\n";
        let cfg = SshConfig::parse_str(text, &t.0.join(".ssh/config"), &t.ctx());
        let work = cfg.resolve("work", &t.ctx());
        assert_eq!(work.user, "worker", "included lines inherit the block until their own Host");
        assert_eq!(work.port, 2200, "the including block resumes after the Include");
        assert_eq!(work.host_key_alias.as_deref(), Some("after"));
        // `other` enters work.conf active, then `Host other` there matches too; first value (worker) still wins.
        let other = cfg.resolve("other", &t.ctx());
        assert_eq!(other.user, "worker");
        assert_eq!(other.host_key_alias, None);
        // A host outside the block gets nothing from the include.
        let lonely = cfg.resolve("lonely", &t.ctx());
        assert_eq!((lonely.user.as_str(), lonely.port, lonely.connect_timeout), ("me", 22, Some(3)));
    }

    #[test]
    fn include_from_an_inactive_block_never_matches_its_own_hosts() {
        let t = TempDir::new("include-never");
        t.write(".ssh/inner.conf", "Host inner\n  User leaked\nMatch all\n  Port 9999\n");
        let text = "Host outer\n  Include inner.conf\nHost *\n  HostKeyAlias star\n";
        let cfg = SshConfig::parse_str(text, &t.0.join(".ssh/config"), &t.ctx());
        let r = cfg.resolve("inner", &t.ctx());
        assert_eq!((r.user.as_str(), r.port), ("me", 22));
        assert_eq!(r.host_key_alias.as_deref(), Some("star"), "state is restored after the Include");
        // From the top level the same file is read normally.
        let top = SshConfig::parse_str("Include inner.conf\n", &t.0.join(".ssh/config"), &t.ctx());
        assert_eq!(top.resolve("inner", &t.ctx()).user, "leaked");
        assert_eq!(top.resolve("other", &t.ctx()).port, 9999);
    }

    #[test]
    fn include_depth_is_limited_to_sixteen_levels() {
        let t = TempDir::new("include-depth");
        for i in 1..=17 {
            let body = match i {
                16 => "Port 2216\nInclude d17.conf\n".to_string(),
                17 => "ConnectTimeout 17\n".to_string(),
                _ => format!("Include d{}.conf\n", i + 1),
            };
            t.write(&format!(".ssh/d{i}.conf"), &body);
        }
        let cfg = SshConfig::parse_str("Include d1.conf\n", &t.0.join(".ssh/config"), &t.ctx());
        let r = cfg.resolve("h", &t.ctx());
        assert_eq!(r.port, 2216, "sixteen levels still load");
        assert_eq!(r.connect_timeout, None, "the seventeenth does not");
        assert_eq!(cfg.warnings().len(), 1, "{:?}", cfg.warnings());
        assert!(cfg.warnings()[0].contains("d16.conf:2"), "{:?}", cfg.warnings());
    }

    #[test]
    fn include_cycles_stop_with_a_warning() {
        let t = TempDir::new("include-cycle");
        t.write(".ssh/a.conf", "IdentityFile a\nInclude b.conf\n");
        t.write(".ssh/b.conf", "IdentityFile b\nInclude a.conf\n");
        let main = t.write(".ssh/config", "Include a.conf\n");
        let cfg = SshConfig::load(&main, &t.ctx());
        let files = cfg.resolve("h", &t.ctx()).identity_files;
        assert_eq!(files, vec![PathBuf::from("a"), PathBuf::from("b")]);
        assert_eq!(cfg.warnings().len(), 1);
        assert!(cfg.warnings()[0].contains("cycle"));
    }

    #[test]
    fn load_of_a_missing_file_is_an_empty_config() {
        let t = TempDir::new("load-missing");
        let cfg = SshConfig::load(&t.0.join("nope"), &t.ctx());
        assert!(cfg.hosts().is_empty());
        assert!(cfg.warnings().is_empty());
        assert_eq!(cfg.resolve("h", &t.ctx()).port, 22);
        assert_eq!(t.ctx().user_config(), t.0.join(".ssh").join("config"));
    }

    #[test]
    fn identity_files_accumulate_and_expand() {
        let text = "Host box\n  HostName 10.0.0.5\n  User deploy\n  Port 2222\n  IdentityFile ~/.ssh/id_%r\n  IdentityFile /keys/%h-%p\n  IdentityFile %d/k/%n\nHost *\n  IdentityFile ~/.ssh/id_ed25519\n  IdentityFile /keys/%u-%%-%x\n";
        let r = resolve(text, "box");
        let expected: Vec<PathBuf> = [
            "/home/me/.ssh/id_deploy",
            "/keys/10.0.0.5-2222",
            "/home/me/k/box",
            "/home/me/.ssh/id_ed25519",
            "/keys/me-%-%x",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(r.identity_files, expected);
    }

    #[test]
    fn tokens_use_the_final_hostname_user_and_port() {
        // The tokens come after HostName/User/Port here, but the values may also be set later in the file.
        let text = "Host box\n  IdentityAgent ~/agents/%r@%h\n  UserKnownHostsFile ~/kh/%h_%p ~/kh/second\n  GlobalKnownHostsFile /g/%n\n  ProxyCommand ssh -W \"%h:%p\" -l %r jump-%u\n  User late\n  HostName late.example\n  Port 2345\n";
        let r = resolve(text, "box");
        assert_eq!(r.identity_agent.as_deref(), Some("/home/me/agents/late@late.example"));
        assert_eq!(r.user_known_hosts_files, vec![PathBuf::from("/home/me/kh/late.example_2345"), PathBuf::from("/home/me/kh/second")]);
        assert_eq!(r.global_known_hosts_files, vec![PathBuf::from("/g/box")]);
        assert_eq!(r.proxy_command.as_deref(), Some("ssh -W \"late.example:2345\" -l late jump-me"));
    }

    #[test]
    fn hostname_only_expands_h_and_percent() {
        assert_eq!(resolve("Host web\n  HostName %h.internal\n", "web").hostname, "web.internal");
        assert_eq!(resolve("HostName a%%b%p\n", "x").hostname, "a%b%p");
    }

    #[test]
    fn known_hosts_files_can_be_none_or_listed() {
        let r = resolve("UserKnownHostsFile none\nGlobalKnownHostsFile /one /two\n", "h");
        assert!(r.user_known_hosts_files.is_empty());
        assert_eq!(r.global_known_hosts_files, vec![PathBuf::from("/one"), PathBuf::from("/two")]);
        let r = resolve("UserKnownHostsFile \"/with space/kh\"\n", "h");
        assert_eq!(r.user_known_hosts_files, vec![PathBuf::from("/with space/kh")]);
    }

    #[test]
    fn identity_agent_none_stays_none() {
        assert_eq!(resolve("IdentityAgent none\n", "h").identity_agent.as_deref(), Some("none"));
    }

    #[test]
    fn proxy_jump_chains_and_ipv6() {
        let r = resolve("ProxyJump alice@jump1:2222,[fe80::1]:22,ssh://bob@jump3:2200,bare\n", "h");
        assert_eq!(
            r.proxy_jump,
            vec![dest(Some("alice"), "jump1", Some(2222)), dest(None, "fe80::1", Some(22)), dest(Some("bob"), "jump3", Some(2200)), dest(None, "bare", None)]
        );
        let r = resolve("ProxyJump carol@[2001:db8::7]:2022\n", "h");
        assert_eq!(r.proxy_jump, vec![dest(Some("carol"), "2001:db8::7", Some(2022))]);
        assert!(resolve("ProxyJump none\n", "h").proxy_jump.is_empty());
        assert!(resolve("Host a\n  ProxyJump none\nHost *\n  ProxyJump j\n", "a").proxy_jump.is_empty());
        assert_eq!(resolve("Host a\n  ProxyJump none\nHost *\n  ProxyJump j\n", "b").proxy_jump, vec![dest(None, "j", None)]);
    }

    #[test]
    fn proxy_jump_and_proxy_command_compete() {
        let r = resolve("ProxyJump j\nProxyCommand nc %h %p\n", "h");
        assert_eq!(r.proxy_jump.len(), 1);
        assert_eq!(r.proxy_command, None);
        let r = resolve("ProxyCommand nc %h %p\nProxyJump j\n", "h");
        assert!(r.proxy_jump.is_empty());
        assert_eq!(r.proxy_command.as_deref(), Some("nc h 22"));
        assert_eq!(resolve("ProxyCommand none\n", "h").proxy_command, None);
        assert_eq!(resolve("ProxyCommand=\"/bin/my proxy\" %h\n", "h").proxy_command.as_deref(), Some("\"/bin/my proxy\" h"));
    }

    #[test]
    fn invalid_proxy_jump_entries_are_dropped_with_a_warning() {
        let c = parse("ProxyJump good,:9,bad:port\n");
        assert_eq!(c.resolve("h", &fake_ctx()).proxy_jump, vec![dest(None, "good", None)]);
        assert_eq!(c.warnings().len(), 2, "{:?}", c.warnings());
    }

    #[test]
    fn parse_destination_formats() {
        assert_eq!(parse_destination("host"), Some(dest(None, "host", None)));
        assert_eq!(parse_destination("host:2222"), Some(dest(None, "host", Some(2222))));
        assert_eq!(parse_destination("user@host"), Some(dest(Some("user"), "host", None)));
        assert_eq!(parse_destination("user@host:22"), Some(dest(Some("user"), "host", Some(22))));
        assert_eq!(parse_destination("user@[::1]:2200"), Some(dest(Some("user"), "::1", Some(2200))));
        assert_eq!(parse_destination("[fe80::1]"), Some(dest(None, "fe80::1", None)));
        assert_eq!(parse_destination("::1"), Some(dest(None, "::1", None)));
        assert_eq!(parse_destination("ssh://host"), Some(dest(None, "host", None)));
        assert_eq!(parse_destination("ssh://user@host:2200/"), Some(dest(Some("user"), "host", Some(2200))));
        assert_eq!(parse_destination("ssh://u@[::1]:22"), Some(dest(Some("u"), "::1", Some(22))));
        assert_eq!(parse_destination("  spaced  "), Some(dest(None, "spaced", None)));
        for bad in ["", "   ", "@host", "user@", "host:", "host:0", "host:70000", "host:abc", "[::1", "[::1]x", "[]", "ssh://", "ssh://host/path", "a b"] {
            assert_eq!(parse_destination(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn safe_names_keep_shell_and_option_characters_out() {
        for good in ["lab-01", "10.0.0.5", "::1", "fe80::1%en0", "db.example.com", "deploy", "a_b"] {
            assert!(safe_name(good), "{good}");
        }
        for bad in ["", "-oProxyCommand=x", "a b", "a;b", "a&b", "a|b", "$(id)", "`id`", "a'b", "a\"b", "a\\b", "a<b", "a>b", "a(b", "a{b", "a\nb"] {
            assert!(!safe_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn hosts_lists_named_aliases_once_in_file_order() {
        let t = TempDir::new("hosts");
        t.write(".ssh/more.conf", "# c\nHost zeta alpha\n  User z\nHost web\n");
        let text = "Host web *.example.com !bad db?\n  User x\nHost db prod\nInclude more.conf\nHost last\n";
        let cfg = SshConfig::parse_str(text, Path::new("main.conf"), &t.ctx());
        let main = PathBuf::from("main.conf");
        let more = t.0.join(".ssh/more.conf");
        let host = |alias: &str, file: &PathBuf, line| ConfigHost { alias: alias.into(), file: file.clone(), line };
        assert_eq!(
            cfg.hosts(),
            vec![host("web", &main, 1), host("db", &main, 3), host("prod", &main, 3), host("zeta", &more, 2), host("alpha", &more, 2), host("last", &main, 5)]
        );
    }

    #[test]
    fn invalid_numbers_are_ignored_with_a_warning() {
        let c = parse("Port 0\nPort 70000\nPort abc\nPort 2200\nConnectTimeout soon\nServerAliveInterval -1\nServerAliveCountMax x\n");
        let r = c.resolve("h", &fake_ctx());
        assert_eq!(r.port, 2200, "a bad Port does not use up the first-value slot");
        assert_eq!((r.connect_timeout, r.server_alive_interval, r.server_alive_count_max), (None, None, None));
        assert_eq!(c.warnings().len(), 6, "{:?}", c.warnings());
        assert!(c.warnings()[0].starts_with("test.conf:1:"));
    }

    #[test]
    fn timeouts_accept_openssh_time_formats() {
        let r = resolve("ConnectTimeout 10\nServerAliveInterval 1m30s\nServerAliveCountMax 4\n", "h");
        assert_eq!((r.connect_timeout, r.server_alive_interval, r.server_alive_count_max), (Some(10), Some(90), Some(4)));
        assert_eq!(parse_time("2h"), Some(7200));
        assert_eq!(parse_time("1W"), Some(604_800));
        assert_eq!(parse_time("s"), None);
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn authentication_options() {
        let r = resolve(
            "StrictHostKeyChecking accept-new\nHostKeyAlias shared\nPreferredAuthentications Publickey,Password keyboard-interactive\nPasswordAuthentication NO\nPubkeyAuthentication yes\nChallengeResponseAuthentication no\nKbdInteractiveAuthentication yes\nForwardAgent yes\nIdentitiesOnly YES\n",
            "h",
        );
        assert_eq!(r.strict_host_key_checking, StrictHostKeyChecking::AcceptNew);
        assert_eq!(r.host_key_alias.as_deref(), Some("shared"));
        assert_eq!(r.preferred_authentications, ["publickey", "password", "keyboard-interactive"]);
        assert!(!r.password_authentication && r.pubkey_authentication);
        assert!(!r.kbd_interactive_authentication, "the first of the two aliases wins");
        assert!(r.forward_agent && r.identities_only);
        for (value, expected) in [("yes", StrictHostKeyChecking::Yes), ("No", StrictHostKeyChecking::No), ("off", StrictHostKeyChecking::No), ("ask", StrictHostKeyChecking::Ask)] {
            assert_eq!(resolve(&format!("StrictHostKeyChecking {value}\n"), "h").strict_host_key_checking, expected, "{value}");
        }
        assert!(resolve("ForwardAgent /run/agent.sock\n", "h").forward_agent);
        assert!(!resolve("ForwardAgent no\n", "h").forward_agent);
        assert!(resolve("PasswordAuthentication maybe\n", "h").password_authentication);
    }

    #[test]
    fn result_serializes_with_kebab_case_strictness() {
        let r = resolve("StrictHostKeyChecking accept-new\n", "h");
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["strict_host_key_checking"], "accept-new");
        assert_eq!(json["port"], 22);
    }

    #[test]
    fn split_args_handles_quotes_escapes_and_comments() {
        assert_eq!(split_args("a \"b c\" d#e"), ["a", "b c", "d#e"]);
        assert_eq!(split_args("a # rest"), ["a"]);
        assert_eq!(split_args("\"\" x"), ["", "x"]);
        assert_eq!(split_args(r#"a\"b C:\dir"#), ["a\"b", "C:\\dir"]);
        assert_eq!(split_args("\"open"), ["open"]);
    }

    #[test]
    fn wildcard_and_glob_matching() {
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("a*c", "abbbc"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("a?c", "ac"));
        assert!(wildcard_match("*.example.*", "x.example.com"));
        assert!(!wildcard_match("abc", "abcd"));
        assert!(wildcard_match("[x]*", "[x]:22"), "brackets are literal for hosts");
        let g = |p: &str, n: &str| segment_matches(p, n);
        assert!(g("[a-c]x", "bx"));
        assert!(!g("[a-c]x", "dx"));
        assert!(g("[!a]x", "bx"));
        assert!(!g("[!a]x", "ax"));
        assert!(g("[]]x", "]x"));
        assert!(g("[x", "[x"), "an unclosed class is literal");
        assert!(!g("*", ".hidden"));
        assert!(g(".*", ".hidden"));
    }
}
