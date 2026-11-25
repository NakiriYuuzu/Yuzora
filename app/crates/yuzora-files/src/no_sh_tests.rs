//! Reads on a Windows machine with no `sh` (#294): hc there refuses to start `sh`, and the
//! Files sidebar and its panes must still list, stat, read, show and find. Each machine's
//! PowerShell is a stand-in (`remote_write::testing::windows_machine`) that prints what the
//! script it is given prints there.

use crate::fsops::{self, Entry, Eol, Listing, Stamp};
use crate::remote_write::testing::windows_machine;
use crate::search::{self, FileList};

fn entry(name: &str, dir: bool, link: bool, size: u64) -> Entry {
    Entry {
        name: name.to_owned(),
        dir,
        link,
        hidden: name.starts_with('.'),
        size,
    }
}

#[test]
fn a_folder_on_a_machine_without_sh_is_listed() {
    windows_machine(
        "win-list",
        r"printf 'f\0a b.txt\0l\0link\0d\0src\0f\0.env\0-\0'; printf '3\n12\n'",
    );
    assert_eq!(
        fsops::list_dir(r"C:\proj", Some("win-list"), 100),
        Ok(Listing {
            entries: vec![
                entry("src", true, false, 0),
                entry(".env", false, false, 12),
                entry("a b.txt", false, false, 3),
                entry("link", false, true, 0),
            ],
            truncated: false,
            skipped: false,
        })
    );
}

#[test]
fn a_file_on_a_machine_without_sh_is_looked_at_and_one_not_there_is_none() {
    windows_machine("win-stat", r"printf '5 1700000000\n'");
    windows_machine("win-stat-gone", "exit 4");
    assert_eq!(
        fsops::stat(r"C:\proj\a.txt", Some("win-stat")),
        Ok(Some(Stamp {
            len: 5,
            mtime_ms: 1_700_000_000_000
        }))
    );
    assert_eq!(fsops::stat(r"C:\proj\gone.txt", Some("win-stat-gone")), Ok(None));
}

#[test]
fn a_file_on_a_machine_without_sh_is_read_and_opens_read_only() {
    windows_machine("win-read", r"printf '6 1700000000 r\n'; printf 'a\r\nb\n'");
    let file = fsops::read_text(r"C:\proj\a.txt", Some("win-read"), 1 << 20).unwrap();
    assert_eq!(
        (file.text.as_str(), file.eol, file.size, file.readonly),
        ("a\nb\n", Eol::Crlf, 6, true)
    );
    assert_eq!(file.stamp.mtime_ms, 1_700_000_000_000);
}

#[test]
fn an_image_on_a_machine_without_sh_is_read() {
    windows_machine("win-image", r"printf 'GIF89a'");
    assert_eq!(
        crate::image::read(r"C:\pics\a.gif", Some("win-image")),
        Ok(b"GIF89a".to_vec())
    );
}

#[test]
fn files_under_a_folder_on_a_machine_without_sh_are_found() {
    windows_machine(
        "win-find",
        r"printf 'find\0a.txt\0src/b.rs\0.hidden\0'",
    );
    assert_eq!(
        search::list_files(r"C:\proj", Some("win-find"), false, 100),
        Ok(FileList {
            files: vec!["a.txt".into(), "src/b.rs".into()],
            truncated: false,
            unfiltered: true,
            skipped: false,
        })
    );
}

/// What the machine has is learned once: the next read goes straight to its PowerShell.
#[test]
fn a_machine_without_sh_is_asked_for_sh_once() {
    let log = windows_machine("win-once", r"printf -- '-\0'");
    for _ in 0..2 {
        assert_eq!(
            fsops::list_dir(r"C:\proj", Some("win-once"), 10),
            Ok(Listing::default())
        );
    }
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "sh\npowershell.exe\npowershell.exe\n"
    );
}
