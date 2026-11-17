//! Each test serves an archive from a stand-in for codeload on 127.0.0.1 and
//! runs the real curl of this machine (and its gzip, to build the archives).

use super::*;
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

const TOP: &str = "acme-demo-1a2b3c4";

struct Lab {
    root: PathBuf,
}

impl Lab {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("yz-fetch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["temp", "home", "outside"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("outside/target"), "original").unwrap();
        Lab { root }
    }

    fn plan(&self, link: &str, base: &str) -> Result<Plan, InstallError> {
        let source = crate::source::parse(link).unwrap_or_else(|| panic!("{link}"));
        plan(&source, base, &self.root.join("temp"), &self.root.join("home"))
    }

    /// Everything in the lab but the temp folder: names, kinds and contents.
    fn outside_temp(&self) -> Vec<String> {
        fn walk(dir: &Path, out: &mut Vec<String>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let meta = std::fs::symlink_metadata(e.path()).unwrap();
                let body = if meta.is_file() { std::fs::read_to_string(e.path()).unwrap_or_default() } else { String::new() };
                out.push(format!("{} {:?} {body}", e.path().display(), meta.file_type()));
                if meta.is_dir() {
                    walk(&e.path(), out);
                }
            }
        }
        let mut out = Vec::new();
        for d in ["home", "outside"] {
            walk(&self.root.join(d), &mut out);
        }
        out
    }

    fn temp_is_empty(&self) -> bool {
        std::fs::read_dir(self.root.join("temp")).unwrap().next().is_none()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A stand-in for codeload: `answer` writes the whole HTTP answer; the paths asked are kept.
fn serve(answer: impl Fn(&str, &mut TcpStream) + Send + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let path = String::from_utf8_lossy(&head).split(' ').nth(1).unwrap_or_default().to_owned();
            log.lock().unwrap().push(path.clone());
            answer(&path, &mut stream);
        }
    });
    (base, asked)
}

fn serving(archive: Vec<u8>) -> (String, Arc<Mutex<Vec<String>>>) {
    serve(move |_, s| {
        let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", archive.len()).as_bytes());
        let _ = s.write_all(&archive);
    })
}

fn header(name: &str, kind: u8, size: usize, link: &str) -> Vec<u8> {
    let mut h = vec![0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[100..108].copy_from_slice(b"0000644\0");
    h[108..116].copy_from_slice(b"0000000\0");
    h[116..124].copy_from_slice(b"0000000\0");
    h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0");
    h[156] = kind;
    h[157..157 + link.len()].copy_from_slice(link.as_bytes());
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    sign(&mut h);
    h
}

fn sign(h: &mut [u8]) {
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
}

/// A tar stream of `(name, type flag, data, link target)`.
fn tar_of(members: &[(&str, u8, &[u8], &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, kind, data, link) in members {
        out.extend(header(name, *kind, data.len(), link));
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    out
}

/// A pax header that names the next member `path`.
fn pax_path(path: &str) -> Vec<u8> {
    let record = |n: usize| format!("{n} path={path}\n");
    let mut n = record(0).len();
    while record(n).len() != n {
        n = record(n).len();
    }
    record(n).into_bytes()
}

/// gzip of what `write` puts on its stdin.
fn gz(write: impl FnOnce(&mut std::process::ChildStdin) + Send + 'static) -> Vec<u8> {
    let mut child = Command::new("gzip").arg("-c").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || write(&mut stdin));
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    out.stdout
}

fn gz_of(tar: Vec<u8>) -> Vec<u8> {
    gz(move |w| w.write_all(&tar).unwrap())
}

/// A zstd frame holding `data` as one raw block (valid zstd, nothing compressed).
fn zstd_of(data: &[u8]) -> Vec<u8> {
    let len = data.len() as u32;
    [&[0x28, 0xb5, 0x2f, 0xfd, 0xa0][..], &len.to_le_bytes(), &(len << 3 | 1).to_le_bytes()[..3], data].concat()
}

/// A zip holding one file, stored.
fn zip_of(name: &str, data: &[u8]) -> Vec<u8> {
    let mut crc = flate2::Crc::new();
    crc.update(data);
    let len = (data.len() as u32).to_le_bytes();
    // version 20, no flags, stored, 1980-01-01, the crc, both sizes, the name's length, no extra field
    let common = [&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0][..], &crc.sum().to_le_bytes(), &len, &len, &(name.len() as u16).to_le_bytes(), &[0, 0]].concat();
    let local = [&0x0403_4b50u32.to_le_bytes()[..], &common, name.as_bytes(), data].concat();
    let central = [&0x0201_4b50u32.to_le_bytes()[..], &[20, 0], &common, &[0; 14], name.as_bytes()].concat();
    let end = [&0x0605_4b50u32.to_le_bytes()[..], &[0, 0, 0, 0, 1, 0, 1, 0], &(central.len() as u32).to_le_bytes(), &(local.len() as u32).to_le_bytes(), &[0, 0]].concat();
    [local, central, end].concat()
}

fn names(list: &[(PathBuf, String)]) -> Vec<&str> {
    list.iter().map(|(_, n)| n.as_str()).collect()
}

#[test]
fn reads_githubs_own_archive() {
    // GitHub's legacy archive of octocat/Hello-World at HEAD, as codeload answered it on 2026-11-17.
    let tar = plain_tar(include_bytes!("octocat-hello-world.tgz")).unwrap();
    assert_eq!(members(&tar).unwrap(), [Member::Folder("octocat-Hello-World-7fd1a60".into()), Member::File("octocat-Hello-World-7fd1a60/README".into(), b"Hello World!\n")]);
}

#[test]
fn plans_a_mod_at_the_root_of_a_repo() {
    let lab = Lab::new("root");
    let archive = gz_of(tar_of(&[(&format!("{TOP}/"), b'5', b"", ""), (&format!("{TOP}/main.js"), b'0', b"exec('git')", ""), (&format!("{TOP}/README.md"), b'0', b"hi", "")]));
    let (base, asked) = serving(archive);
    let before = lab.outside_temp();
    let p = lab.plan("https://github.com/acme/demo", &base).unwrap();
    assert_eq!((p.kind, names(&p.mods)), ("mod", vec!["demo"]));
    assert_eq!(p.to_json()["source"], "https://github.com/acme/demo");
    assert_eq!(p.to_json()["commit"], "1a2b3c4");
    assert_eq!(p.reaches, ["programs"]);
    assert_eq!(*asked.lock().unwrap(), ["/acme/demo/legacy.tar.gz/HEAD"]);
    assert!(lab.temp_is_empty(), "the temp folder is gone");
    assert_eq!(lab.outside_temp(), before, "planning writes nothing to the home");
    crate::mods::write(&p, &lab.root.join("home")).unwrap();
    assert_eq!(std::fs::read_to_string(lab.root.join("home/mods/demo/main.js")).unwrap(), "exec('git')");
}

#[test]
fn plans_the_folder_a_herdr_dev_link_names() {
    let lab = Lab::new("herdr");
    let top = "acme-kit-0123456789abcdef0123456789abcdef01234567";
    let long = pax_path(&format!("{top}/release/blackx/mods/side/{}.js", "x".repeat(90)));
    let archive = gz_of(tar_of(&[
        (&format!("{top}/release/blackx/layouts/deck.json"), b'0', br#"{"mod":"side"}"#, ""),
        (&format!("{top}/release/blackx/mods/side/main.js"), b'0', b"side", ""),
        ("pax", b'x', &long, ""),
        ("long", b'0', b"far down", ""),
        (&format!("{top}/release/other/mods/x/main.js"), b'0', b"not this one", ""),
        (&format!("{top}/mods/y/main.js"), b'0', b"nor this", ""),
    ]));
    let (base, asked) = serving(archive);
    let p = lab.plan("herdr.dev/layouts/acme/kit--release--blackx", &base).unwrap();
    assert_eq!(*asked.lock().unwrap(), ["/acme/kit/legacy.tar.gz/HEAD"], "GitHub's archive of acme/kit, nothing else");
    assert_eq!((p.kind, names(&p.mods), names(&p.layouts)), ("bundle", vec!["side"], vec!["deck"]));
    assert_eq!(p.to_json()["source"], "https://github.com/acme/kit/tree/HEAD/release/blackx");
    assert_eq!(p.to_json()["commit"], "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(p.files, 3, "the layout, the mod and its long-named file");
    assert!(lab.temp_is_empty());
}

#[test]
fn a_repo_without_a_mod_is_not_a_mod() {
    let lab = Lab::new("none");
    let (base, _) = serving(gz_of(tar_of(&[(&format!("{TOP}/README.md"), b'0', b"docs only", "")])));
    assert!(matches!(lab.plan("acme/demo", &base), Err(InstallError::NotAMod)));
    let missing = "https://github.com/acme/demo/tree/main/no/such/folder";
    assert!(matches!(lab.plan(missing, &base), Err(InstallError::NotAMod)));
    assert!(lab.temp_is_empty());
}

#[test]
fn links_and_devices_are_left_out_and_nothing_leaves_the_folder() {
    let lab = Lab::new("links");
    let outside = lab.root.join("outside");
    let target = outside.join("target").to_string_lossy().into_owned();
    let archive = gz_of(tar_of(&[
        (&format!("{TOP}/main.js"), b'0', b"m", ""),
        (&format!("{TOP}/link"), b'2', b"", &outside.to_string_lossy()),
        (&format!("{TOP}/link/pwned"), b'0', b"through the link", ""),
        (&format!("{TOP}/rel"), b'2', b"", "../../outside"),
        (&format!("{TOP}/rel/pwned"), b'0', b"through the link", ""),
        (&format!("{TOP}/f"), b'2', b"", &target),
        (&format!("{TOP}/f"), b'0', b"over the link", ""),
        (&format!("{TOP}/hl"), b'1', b"", &target),
        (&format!("{TOP}/hl2"), b'1', b"", "../outside/target"),
        (&format!("{TOP}/hl2"), b'0', b"over the hard link", ""),
        (&format!("{TOP}/null"), b'3', b"", ""),
        (&format!("{TOP}/fifo"), b'6', b"", ""),
    ]));
    let (base, _) = serving(archive);
    let before = lab.outside_temp();
    let planned = lab.plan("acme/demo", &base);
    assert_eq!(lab.outside_temp(), before, "nothing outside the temp folder was made or changed");
    let p = planned.unwrap();
    let mut written: Vec<String> = p.pieces.iter().map(|(to, b)| format!("{} {}", to.to_string_lossy(), String::from_utf8_lossy(b))).collect();
    written.sort();
    assert_eq!(written, ["mods/demo/f over the link", "mods/demo/hl2 over the hard link", "mods/demo/link/pwned through the link", "mods/demo/main.js m", "mods/demo/rel/pwned through the link"], "files only, as plain files of the folder");
    assert!(lab.temp_is_empty());
}

/// The reader alone, on each way a long path is written: pax (GitHub), GNU `L`, and the ustar `prefix` field (what
/// this machine's tar may turn the first into before the reader sees it).
#[test]
fn reads_long_names_however_they_are_written_and_refuses_the_ones_that_leave() {
    let folder = format!("{TOP}/{}", "d".repeat(120));
    let long = format!("{folder}/main.js");
    let mut prefixed = header("main.js", b'0', 1, "");
    prefixed[345..345 + folder.len()].copy_from_slice(folder.as_bytes());
    sign(&mut prefixed);
    prefixed.extend(b"m");
    prefixed.resize(512 * 4, 0);
    let gnu = format!("{long}\0");
    for tar in [
        tar_of(&[("pax", b'x', &pax_path(&long), ""), ("short", b'0', b"m", "")]),
        tar_of(&[("././@LongLink", b'L', gnu.as_bytes(), ""), ("short", b'0', b"m", "")]),
        prefixed,
    ] {
        assert_eq!(members(&tar), Ok(vec![Member::File(long.clone(), b"m")]));
    }
    for name in ["/etc/x", &format!("{TOP}//x"), &format!("{TOP}/../x"), &format!("{TOP}/a\\b"), &format!("{TOP}/a\nb.js")] {
        assert!(members(&tar_of(&[(name, b'0', b"m", "")])).is_err(), "{name}");
        assert!(members(&tar_of(&[("pax", b'x', &pax_path(name), ""), ("short", b'0', b"m", "")])).is_err(), "pax {name}");
    }
}

#[test]
fn a_member_that_climbs_out_fails() {
    let lab = Lab::new("climb");
    let absolute = lab.root.join("outside/abs").to_string_lossy().into_owned();
    for member in [format!("{TOP}/../../outside/pwned"), format!("{TOP}/./main.js"), absolute.clone(), format!("{TOP}/a\\..\\b")] {
        let (base, _) = serving(gz_of(tar_of(&[(&format!("{TOP}/main.js"), b'0', b"m", ""), (&member, b'0', b"pwned", "")])));
        let before = lab.outside_temp();
        assert!(matches!(lab.plan("acme/demo", &base), Err(InstallError::UnpackFailed(_))), "{member}");
        assert_eq!(lab.outside_temp(), before, "{member}");
        assert!(lab.temp_is_empty(), "{member}");
    }
    assert!(!Path::new(&absolute).exists());
}

#[test]
fn an_archive_that_is_not_githubs_fails() {
    let lab = Lab::new("shape");
    let cases: [&[(&str, u8, &[u8], &str)]; 5] = [
        &[("acme-demo-1a2b3c4/main.js", b'0', b"m", ""), ("other/main.js", b'0', b"m", "")],
        &[("acme-demo-1a2b3c4/main.js", b'0', b"m", ""), ("loose.js", b'0', b"m", "")],
        &[("acme-deco-1a2b3c4/main.js", b'0', b"another repo's", "")],
        &[("acme-demo-HEAD/main.js", b'0', b"m", "")],
        &[],
    ];
    for members in cases {
        let (base, _) = serving(gz_of(tar_of(members)));
        assert!(matches!(lab.plan("acme/demo", &base), Err(InstallError::UnpackFailed(_))), "{members:?}");
        assert!(lab.temp_is_empty());
    }
    let (base, _) = serving(b"<html>not an archive</html>".to_vec());
    assert!(matches!(lab.plan("acme/demo", &base), Err(InstallError::UnpackFailed(_))));
}

#[test]
fn a_redirect_is_not_followed() {
    let lab = Lab::new("redirect");
    let (base, asked) = serve(|_, s| {
        let _ = s.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.2:9/elsewhere.tgz\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    });
    match lab.plan("acme/demo", &base) {
        Err(InstallError::FetchFailed(why)) => assert!(why.contains("302") && why.contains("127.0.0.2:9"), "{why}"),
        other => panic!("{:?}", other.map(|p| p.to_json())),
    }
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(lab.temp_is_empty());
}

#[test]
fn a_missing_repo_or_no_server_fails_to_fetch() {
    let lab = Lab::new("missing");
    let (base, _) = serve(|_, s| {
        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    });
    match lab.plan("acme/demo", &base) {
        Err(InstallError::FetchFailed(why)) => assert_eq!(why, "HTTP 404", "GitHub's answer"),
        other => panic!("{:?}", other.map(|p| p.to_json())),
    }
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    assert!(matches!(lab.plan("acme/demo", &format!("http://{closed}")), Err(InstallError::FetchFailed(_))));
    assert!(lab.temp_is_empty());
}

#[test]
fn a_download_cut_short_says_why() {
    let lab = Lab::new("cut");
    let (base, _) = serve(|_, s| {
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\nonly this much");
    });
    match lab.plan("acme/demo", &base) {
        Err(InstallError::FetchFailed(why)) => assert!(why.starts_with("curl: (18)") && !why.contains("HTTP 200"), "{why}"),
        other => panic!("{:?}", other.map(|p| p.to_json())),
    }
    assert!(lab.temp_is_empty());
}

/// What bsdtar reads besides a gzip of a tar: handed one as the body, it planned each (the mtree one with a file
/// of this computer's as main.js). Read as a gzip of a tar and nothing else, none is planned.
#[test]
fn a_body_that_is_not_a_gzip_of_a_tar_is_refused() {
    let lab = Lab::new("formats");
    let local = lab.root.join("outside/target").to_string_lossy().into_owned();
    let mtree = format!("#mtree\n{TOP} type=dir\n{TOP}/main.js type=file contents={local}\n").into_bytes();
    let tar = tar_of(&[(&format!("{TOP}/main.js"), b'0', b"m", "")]);
    let bodies = [("gzipped mtree", gz_of(mtree.clone())), ("mtree", mtree), ("zstd", zstd_of(&tar)), ("zip", zip_of(&format!("{TOP}/main.js"), b"m"))];
    let mut planned = Vec::new();
    for (what, body) in bodies {
        let (base, _) = serving(body);
        match lab.plan("acme/demo", &base) {
            Err(InstallError::UnpackFailed(_)) => {}
            other => planned.push(format!("{what}: {:?}", other.map(|p| p.to_json()))),
        }
        assert!(lab.temp_is_empty(), "{what}");
    }
    assert!(planned.is_empty(), "{planned:#?}");
}

#[test]
fn a_download_over_the_limit_is_too_big() {
    let lab = Lab::new("big");
    // No length up front: a stream that just goes on.
    let (base, _) = serve(|_, s| {
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
        let chunk = vec![0u8; 1 << 20];
        for _ in 0..(MAX_DOWNLOAD >> 20) + 2 {
            if s.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).and_then(|()| s.write_all(&chunk)).and_then(|()| s.write_all(b"\r\n")).is_err() {
                return;
            }
        }
        let _ = s.write_all(b"0\r\n\r\n");
    });
    assert!(matches!(lab.plan("acme/demo", &base), Err(InstallError::TooBig)));
    assert!(lab.temp_is_empty());
}

#[test]
fn a_small_archive_that_unpacks_huge_is_too_big() {
    let lab = Lab::new("bomb");
    let size = (MAX_UNPACKED + (1 << 20)) as usize;
    let small = tar_of(&[(&format!("{TOP}/mod/main.js"), b'0', b"m", "")]);
    let archive = gz(move |w| {
        w.write_all(&small[..small.len() - 1024]).unwrap();
        w.write_all(&header(&format!("{TOP}/big.bin"), b'0', size, "")).unwrap();
        let zeros = vec![0u8; 1 << 20];
        for _ in 0..size >> 20 {
            w.write_all(&zeros).unwrap();
        }
        w.write_all(&[0u8; 1024]).unwrap();
    });
    assert!(archive.len() < 1 << 20, "a small download: {}", archive.len());
    let (base, _) = serving(archive);
    // The huge file is outside the folder the link names, so nothing after the unpack step would see it.
    assert!(matches!(lab.plan("https://github.com/acme/demo/tree/HEAD/mod", &base), Err(InstallError::TooBig)));
    assert!(lab.temp_is_empty());
}

#[test]
fn a_temp_name_already_taken_is_never_used() {
    let lab = Lab::new("taken");
    let root = lab.root.join("temp");
    let next = NEXT_TEMP.load(Ordering::Relaxed);
    for n in next..next + 64 {
        std::os::unix::fs::symlink(lab.root.join("outside"), root.join(format!("yuzora-fetch-{}-{n}", std::process::id()))).unwrap();
    }
    let temp = temp_in(&root).unwrap();
    let meta = std::fs::symlink_metadata(&temp.0).unwrap();
    assert!(meta.is_dir(), "a folder of its own, not the link");
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777, 0o700, "no one else can look in");
    drop(temp);
    assert_eq!(std::fs::read_to_string(lab.root.join("outside/target")).unwrap(), "original");
}
