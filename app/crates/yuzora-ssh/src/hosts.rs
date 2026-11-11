//! Where hosts come from, merged into one list: `~/.ssh/config` (named Host
//! entries), Yuzora's own profiles (settings `ssh.profiles`), and herdr
//! machines (`hc machine ls`, handed in by the script that asked).
//!
//! Ids: `ssh:<alias>`, `profile:<id>`, `machine:<name>`. [`lookup`] also takes
//! a bare alias, profile name, machine name or `user@host:port`, so another
//! lane can pass whatever it stored.

use crate::config::{JumpHost, ResolvedHost, SshConfig, parse_destination, safe_name};
use crate::context::{Context, SECTION};
use crate::error::{ErrorKind, Result, SshError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// A profile kept by Yuzora (settings `ssh.profiles`); its password lives in
/// the credential store, never here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}

/// One herdr machine, as `hc machine ls --json` lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Machine {
    pub name: String,
    pub target: Option<String>,
    pub state: Option<String>,
    pub temporary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Config,
    Profile,
    Machine,
    /// A destination typed or passed in (`user@host:port`).
    Direct,
}

/// A host with everything needed to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDef {
    pub id: String,
    pub name: String,
    pub source: Source,
    pub resolved: ResolvedHost,
    /// The herdr machine that reaches the same host, if any.
    pub machine: Option<Machine>,
    pub profile: Option<Profile>,
}

impl HostDef {
    /// `user@host`, with `:port` when not 22.
    pub fn target(&self) -> String {
        target(&self.resolved.user, &self.resolved.hostname, self.resolved.port)
    }

    /// The argv that opens this host in a terminal with OpenSSH.
    pub fn ssh_argv(&self) -> Vec<String> {
        match self.source {
            // ssh reads the same config, so the alias says everything.
            Source::Config => vec!["ssh".into(), "--".into(), self.name.clone()],
            _ => {
                let r = &self.resolved;
                let mut argv = vec!["ssh".to_owned()];
                if r.port != 22 {
                    argv.extend(["-p".into(), r.port.to_string()]);
                }
                if let Some(p) = &self.profile {
                    if let Some(key) = p.identity_file.as_deref().filter(|k| !k.is_empty()) {
                        argv.extend(["-i".into(), key.to_owned()]);
                    }
                }
                if !r.proxy_jump.is_empty() {
                    argv.extend(["-J".into(), r.proxy_jump.iter().map(jump_text).collect::<Vec<_>>().join(",")]);
                }
                argv.push("--".into());
                argv.push(if r.user.is_empty() { r.hostname.clone() } else { format!("{}@{}", r.user, r.hostname) });
                argv
            }
        }
    }

    /// The listing's JSON (state fields are added by the manager).
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "source": self.source,
            "target": self.target(),
            "hostname": self.resolved.hostname,
            "user": self.resolved.user,
            "port": self.resolved.port,
            "proxyJump": (!self.resolved.proxy_jump.is_empty()).then(|| self.resolved.proxy_jump.iter().map(jump_text).collect::<Vec<_>>().join(",")),
            "machine": self.machine.as_ref().map(|m| m.name.clone()),
            "machineState": self.machine.as_ref().and_then(|m| m.state.clone()),
            "editable": self.source == Source::Profile,
            "profile": self.profile,
        })
    }
}

pub fn target(user: &str, host: &str, port: u16) -> String {
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_owned() };
    let base = if user.is_empty() { host } else { format!("{user}@{host}") };
    if port == 22 { base } else { format!("{base}:{port}") }
}

pub fn jump_text(j: &JumpHost) -> String {
    let host = if j.host.contains(':') { format!("[{}]", j.host) } else { j.host.clone() };
    let base = match &j.user {
        Some(u) => format!("{u}@{host}"),
        None => host,
    };
    match j.port {
        Some(p) => format!("{base}:{p}"),
        None => base,
    }
}

/// The profiles in settings `ssh.profiles`, valid ones only.
pub fn profiles(ctx: &Context) -> Vec<Profile> {
    let section = yuzora_core::settings::section_from(&ctx.settings, SECTION);
    section
        .get("profiles")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(|p| serde_json::from_value::<Profile>(p.clone()).ok()).filter(|p| !p.id.is_empty() && !p.host.trim().is_empty()).collect())
        .unwrap_or_default()
}

pub(crate) fn write_profiles(ctx: &Context, list: &[Profile]) -> Result<()> {
    let mut section = yuzora_core::settings::section_from(&ctx.settings, SECTION);
    if !section.is_object() {
        section = json!({});
    }
    section["profiles"] = serde_json::to_value(list).unwrap_or(Value::Array(Vec::new()));
    yuzora_core::settings::set_section_at(&ctx.settings, SECTION, section).map_err(SshError::io)
}

/// Adds a profile (no id) or replaces the one with its id. Returns its id, which is a new one
/// when the profile was edited to reach another server or account, or to go there another way
/// ([`elsewhere`]).
pub fn save_profile(ctx: &Context, mut p: Profile) -> Result<String> {
    p.host = p.host.trim().to_owned();
    p.name = p.name.trim().to_owned();
    if p.host.is_empty() || p.host.contains(char::is_whitespace) {
        return Err(SshError::text(ErrorKind::Invalid, "profile-host", &[]));
    }
    if !safe_name(&p.host) {
        return Err(SshError::text(ErrorKind::Invalid, "unsafe-name", &[("name", p.host.clone())]));
    }
    if p.name.is_empty() {
        p.name = p.host.clone();
    }
    if p.port == Some(0) {
        return Err(SshError::text(ErrorKind::Invalid, "port", &[]));
    }
    for field in [&mut p.user, &mut p.identity_file, &mut p.proxy_jump] {
        if field.as_deref().is_some_and(|s| s.trim().is_empty()) {
            *field = None;
        }
    }
    if let Some(user) = p.user.as_deref().filter(|u| !safe_name(u)) {
        return Err(SshError::text(ErrorKind::Invalid, "unsafe-name", &[("name", user.to_owned())]));
    }
    let mut list = profiles(ctx);
    // An id no profile has is one a profile had before it was removed or pointed elsewhere:
    // saving under it would bring that id back.
    let at = match p.id.is_empty() {
        true => None,
        false => Some(list.iter().position(|q| q.id == p.id).ok_or_else(|| SshError::text(ErrorKind::UnknownHost, "unknown-host", &[("host", format!("profile:{}", p.id))]))?),
    };
    // A profile that now names another server or account is another host, and gets another id.
    // What knows the old id (an SFTP pane and its listing, a transfer and its `.part`, a file
    // opened for editing) was made on the first server: under the same id it would go on
    // working on the second. Under a new one it finds no host.
    if at.is_none_or(|i| elsewhere(ctx, &list[i], &p)) {
        p.id = new_id(&list);
    }
    match at {
        Some(i) => list[i] = p.clone(),
        None => list.push(p.clone()),
    }
    write_profiles(ctx, &list)?;
    Ok(p.id)
}

/// Whether two profiles reach different servers or accounts, or the same address through other
/// jump hosts (an address behind one bastion is another machine behind the next). As connecting
/// resolves them: what a profile leaves out, `~/.ssh/config` or the defaults fill in.
fn elsewhere(ctx: &Context, a: &Profile, b: &Profile) -> bool {
    let cfg = SshConfig::load(&ctx.ssh_config, &ctx.config);
    let reach = |p: &Profile| {
        let r = from_profile(ctx, &cfg, p);
        (r.user, r.hostname, r.port, r.proxy_jump)
    };
    reach(a) != reach(b)
}

pub fn remove_profile(ctx: &Context, id: &str) -> Result<bool> {
    let mut list = profiles(ctx);
    let before = list.len();
    list.retain(|p| p.id != id);
    if list.len() == before {
        return Ok(false);
    }
    write_profiles(ctx, &list)?;
    Ok(true)
}

/// The time in milliseconds, in hex, and past every id in `list`: a profile saved twice in a
/// millisecond must not get back the id it had the first time (see [`save_profile`]).
fn new_id(list: &[Profile]) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let last = list.iter().filter_map(|p| u128::from_str_radix(&p.id, 16).ok()).max();
    format!("{:x}", last.map_or(now, |n| now.max(n.saturating_add(1))))
}

fn expand_tilde(ctx: &Context, s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => ctx.config.home.join(rest),
        None if s == "~" => ctx.config.home.clone(),
        None => PathBuf::from(s),
    }
}

fn from_profile(ctx: &Context, cfg: &SshConfig, p: &Profile) -> ResolvedHost {
    let mut r = cfg.resolve(&p.host, &ctx.config);
    if let Some(port) = p.port {
        r.port = port;
    }
    if let Some(user) = p.user.as_deref() {
        r.user = user.to_owned();
    }
    if let Some(key) = p.identity_file.as_deref() {
        r.identity_files = vec![expand_tilde(ctx, key)];
    }
    if let Some(jump) = p.proxy_jump.as_deref() {
        r.proxy_jump = if jump.trim().eq_ignore_ascii_case("none") { Vec::new() } else { jump.split(',').filter_map(parse_destination).collect() };
    }
    r
}

fn from_destination(ctx: &Context, cfg: &SshConfig, dest: &JumpHost) -> ResolvedHost {
    let mut r = cfg.resolve(&dest.host, &ctx.config);
    if let Some(user) = &dest.user {
        r.user = user.clone();
    }
    if let Some(port) = dest.port {
        r.port = port;
    }
    r
}

/// The machine reaching `alias` exactly (its target is the alias, maybe with the same user).
fn machine_for<'a>(machines: &'a [Machine], alias: &str) -> Option<&'a Machine> {
    machines.iter().filter(|m| !m.temporary).find(|m| {
        m.name == alias || m.target.as_deref().and_then(parse_destination).is_some_and(|d| d.host == alias && d.port.is_none())
    })
}

/// Every host, in order: ssh config, profiles, then machines no config host covers.
pub fn list(ctx: &Context, machines: &[Machine]) -> Vec<HostDef> {
    let cfg = SshConfig::load(&ctx.ssh_config, &ctx.config);
    let mut out = Vec::new();
    let mut claimed: Vec<String> = Vec::new();
    for h in cfg.hosts() {
        let machine = machine_for(machines, &h.alias).cloned();
        if let Some(m) = &machine {
            claimed.push(m.name.clone());
        }
        out.push(HostDef { id: format!("ssh:{}", h.alias), name: h.alias.clone(), source: Source::Config, resolved: cfg.resolve(&h.alias, &ctx.config), machine, profile: None });
    }
    for p in profiles(ctx) {
        out.push(HostDef { id: format!("profile:{}", p.id), name: p.name.clone(), source: Source::Profile, resolved: from_profile(ctx, &cfg, &p), machine: None, profile: Some(p) });
    }
    for m in machines.iter().filter(|m| !m.temporary && !claimed.contains(&m.name)) {
        let dest = m.target.as_deref().and_then(parse_destination).unwrap_or(JumpHost { user: None, host: m.name.clone(), port: None });
        out.push(HostDef { id: format!("machine:{}", m.name), name: m.name.clone(), source: Source::Machine, resolved: from_destination(ctx, &cfg, &dest), machine: Some(m.clone()), profile: None });
    }
    out
}

/// One host by id, or by alias, profile name, machine name or `user@host:port`.
pub fn lookup(ctx: &Context, machines: &[Machine], key: &str) -> Result<HostDef> {
    let key = key.trim();
    if key.is_empty() {
        return Err(SshError::text(ErrorKind::UnknownHost, "unknown-host", &[("host", key.into())]));
    }
    let all = list(ctx, machines);
    let by = |f: &dyn Fn(&HostDef) -> bool| all.iter().find(|h| f(h)).cloned();
    if let Some(h) = by(&|h| h.id == key)
        .or_else(|| by(&|h| h.source == Source::Config && h.name == key))
        .or_else(|| by(&|h| h.source == Source::Profile && h.name == key))
        .or_else(|| by(&|h| h.machine.as_ref().is_some_and(|m| m.name == key)))
    {
        return Ok(h);
    }
    // An id whose source no longer has it (a removed profile, a renamed alias).
    if key.starts_with("ssh:") || key.starts_with("profile:") || key.starts_with("machine:") {
        return Err(SshError::text(ErrorKind::UnknownHost, "unknown-host", &[("host", key.into())]));
    }
    let raw = key.strip_prefix("dest:").unwrap_or(key);
    let dest = parse_destination(raw).ok_or_else(|| SshError::text(ErrorKind::UnknownHost, "unknown-host", &[("host", key.into())]))?;
    if let Some(bad) = std::iter::once(&dest.host).chain(dest.user.as_ref()).find(|s| !safe_name(s)) {
        return Err(SshError::text(ErrorKind::Invalid, "unsafe-name", &[("name", bad.clone())]));
    }
    let cfg = SshConfig::load(&ctx.ssh_config, &ctx.config);
    Ok(HostDef { id: format!("dest:{raw}"), name: raw.to_owned(), source: Source::Direct, resolved: from_destination(ctx, &cfg, &dest), machine: None, profile: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestWorld;

    #[test]
    fn config_profiles_and_machines_merge_into_one_list() {
        let w = TestWorld::new("hosts-merge");
        w.write_ssh_config("Host lab-01\n  HostName 10.0.0.5\n  User yuzu\nHost pi-*\n  User pi\nHost nas\n  HostName nas.local\n  Port 2222\n");
        let ctx = w.context();
        let id = save_profile(&ctx, Profile { name: "box".into(), host: "box.example".into(), user: Some("root".into()), ..Default::default() }).unwrap();
        let machines = vec![
            Machine { name: "lab-01".into(), target: Some("lab-01".into()), state: Some("online".into()), temporary: false },
            Machine { name: "vps".into(), target: Some("me@vps.example:2200".into()), state: Some("offline".into()), temporary: false },
            Machine { name: "tmp".into(), target: Some("tmp".into()), state: None, temporary: true },
        ];
        let all = list(&ctx, &machines);
        let ids: Vec<&str> = all.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["ssh:lab-01", "ssh:nas", &format!("profile:{id}"), "machine:vps"]);
        assert_eq!(all[0].target(), "yuzu@10.0.0.5");
        assert_eq!(all[0].machine.as_ref().unwrap().name, "lab-01");
        assert_eq!(all[1].target(), format!("{}@nas.local:2222", ctx.config.local_user));
        assert_eq!(all[2].target(), "root@box.example");
        assert_eq!(all[3].target(), "me@vps.example:2200");
        assert_eq!(all[0].ssh_argv(), ["ssh", "--", "lab-01"]);
        assert_eq!(all[3].ssh_argv(), ["ssh", "-p", "2200", "--", "me@vps.example"]);
    }

    #[test]
    fn lookup_takes_ids_names_and_destinations() {
        let w = TestWorld::new("hosts-lookup");
        w.write_ssh_config("Host lab\n  HostName 10.0.0.5\n  User yuzu\n");
        let ctx = w.context();
        assert_eq!(lookup(&ctx, &[], "ssh:lab").unwrap().resolved.hostname, "10.0.0.5");
        assert_eq!(lookup(&ctx, &[], "lab").unwrap().id, "ssh:lab");
        let d = lookup(&ctx, &[], "root@[::1]:2022").unwrap();
        assert_eq!((d.resolved.user.as_str(), d.resolved.hostname.as_str(), d.resolved.port), ("root", "::1", 2022));
        assert_eq!(d.target(), "root@[::1]:2022");
        assert_eq!(lookup(&ctx, &[], "profile:gone").unwrap_err().kind, ErrorKind::UnknownHost);
    }

    #[test]
    fn profiles_round_trip_and_keep_other_settings() {
        let w = TestWorld::new("hosts-profiles");
        let ctx = w.context();
        yuzora_core::settings::set_section_at(&ctx.settings, SECTION, json!({ "preserve": false })).unwrap();
        assert!(save_profile(&ctx, Profile { host: "  ".into(), ..Default::default() }).is_err());
        // Names that would reach a shell (%h in a ProxyCommand) or ssh's options.
        for (host, user) in [("h;touch x", None), ("-oProxyCommand=sh", None), ("h.example", Some("$(id)")), ("h.example", Some("-l"))] {
            let p = Profile { host: host.into(), user: user.map(Into::into), ..Default::default() };
            assert_eq!(save_profile(&ctx, p).unwrap_err().kind, ErrorKind::Invalid, "{host} {user:?}");
        }
        assert!(lookup(&ctx, &[], "dest:me@h`id`.example").is_err());
        let id = save_profile(&ctx, Profile { host: "h.example".into(), user: Some(" ".into()), proxy_jump: Some("jump@bastion:2200".into()), ..Default::default() }).unwrap();
        let p = &profiles(&ctx)[0];
        assert_eq!((p.name.as_str(), p.user.as_deref()), ("h.example", None));
        let h = lookup(&ctx, &[], &format!("profile:{id}")).unwrap();
        assert_eq!(h.resolved.proxy_jump, vec![JumpHost { user: Some("jump".into()), host: "bastion".into(), port: Some(2200) }]);
        assert_eq!(h.ssh_argv(), ["ssh", "-J", "jump@bastion:2200", "--", &format!("{}@h.example", ctx.config.local_user)]);
        assert!(remove_profile(&ctx, &id).unwrap());
        assert!(!remove_profile(&ctx, &id).unwrap());
        assert_eq!(yuzora_core::settings::section_from(&ctx.settings, SECTION)["preserve"], false);
    }

    #[test]
    fn a_profile_keeps_its_id_until_it_reaches_another_server_or_account() {
        let w = TestWorld::new("hosts-profile-id");
        w.write_ssh_config("Host nas\n  HostName nas.local\n  Port 2222\n  User admin\n");
        let ctx = w.context();
        let ids = || profiles(&ctx).into_iter().map(|p| p.id).collect::<Vec<_>>();
        let other = save_profile(&ctx, Profile { host: "other.example".into(), ..Default::default() }).unwrap();
        let mut id = save_profile(&ctx, Profile { name: "box".into(), host: "h.example".into(), user: Some("me".into()), ..Default::default() }).unwrap();
        let mut seen = vec![other.clone(), id.clone()];
        let edit = |id: &str, f: &dyn Fn(&mut Profile)| {
            let mut p = profiles(&ctx).into_iter().find(|p| p.id == id).unwrap();
            f(&mut p);
            save_profile(&ctx, p).unwrap()
        };
        type Edit<'a> = (&'a str, bool, &'a dyn Fn(&mut Profile));
        let edits: [Edit; 15] = [
            // The same server and account, named or signed in to another way.
            ("name", false, &|p| p.name = "renamed".into()),
            ("key", false, &|p| p.identity_file = Some("~/.ssh/other".into())),
            ("port 22 for none", false, &|p| p.port = Some(22)),
            ("no jump, said", false, &|p| p.proxy_jump = Some("none".into())),
            // Another one.
            ("host", true, &|p| p.host = "h2.example".into()),
            ("port", true, &|p| p.port = Some(2200)),
            ("user", true, &|p| p.user = Some("root".into())),
            ("jump", true, &|p| p.proxy_jump = Some("bastion".into())),
            ("another jump", true, &|p| p.proxy_jump = Some("bastion-2".into())),
            // What the profile leaves out is what ~/.ssh/config says, not 22 and the local user.
            ("config host", true, &|p| *p = Profile { id: p.id.clone(), host: "nas".into(), ..Default::default() }),
            ("the config's port", false, &|p| p.port = Some(2222)),
            ("the config's user", false, &|p| p.user = Some("admin".into())),
            ("port 22 for the config's", true, &|p| p.port = Some(22)),
            ("the config's port for 22", true, &|p| p.port = None),
            ("the name the config gives", true, &|p| p.host = "nas.local".into()),
        ];
        for (what, moves, f) in edits {
            let next = edit(&id, f);
            assert_eq!(next != id, moves, "{what}");
            assert_eq!(ids(), [other.clone(), next.clone()], "{what}: in the profile's place");
            if moves {
                assert_eq!(lookup(&ctx, &[], &format!("profile:{id}")).unwrap_err().kind, ErrorKind::UnknownHost, "{what}");
                seen.push(next.clone());
            }
            id = next;
        }
        // No id comes back: a new profile's is past every one there is, whatever the clock says,
        seen.push(save_profile(&ctx, Profile { host: "new.example".into(), ..Default::default() }).unwrap());
        let distinct: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(distinct.len(), seen.len(), "{seen:?}");
        let far = Profile { id: format!("{:x}", u128::MAX / 2), ..Default::default() };
        assert_eq!(new_id(&[far]), format!("{:x}", u128::MAX / 2 + 1));
        // and a profile is not saved under an id none has (one it had before).
        let stale = Profile { id: seen[1].clone(), host: "h.example".into(), ..Default::default() };
        assert_eq!(save_profile(&ctx, stale).unwrap_err().kind, ErrorKind::UnknownHost);
        assert_eq!(ids().len(), 3);
    }
}
