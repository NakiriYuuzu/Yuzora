use super::*;

fn index_entries(root: &Path) -> Vec<u8> {
    let output = run_git(
        root,
        &["ls-files", "--stage", "-z"],
        Duration::from_secs(5),
        &[],
    )
    .unwrap();
    assert_eq!(output.code, 0);
    output.stdout
}

fn check_root_identity(suffix: &str, nested: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let intended = tmp.path().join(format!("repo{suffix}"));
    let sibling = tmp
        .path()
        .join(if suffix.is_empty() { "sibling" } else { "repo" });
    for root in [&intended, &sibling] {
        std::fs::create_dir(root).unwrap();
        test_repo::init(root);
    }
    std::fs::write(intended.join("shared.txt"), "intended contents").unwrap();
    std::fs::write(sibling.join("shared.txt"), "sibling contents").unwrap();
    std::fs::write(sibling.join("canary.txt"), "already staged").unwrap();
    stage(&sibling, &["canary.txt".into()]).unwrap();
    let sibling_index = index_entries(&sibling);
    let target = if nested {
        let child = intended.join("nested");
        std::fs::create_dir(&child).unwrap();
        child
    } else {
        intended.clone()
    };
    let path = target.to_str().unwrap();
    let trust =
        crate::workspace_trust::WorkspaceTrustState::at(tmp.path().join("workspace-trust.json"));
    trust.0.grant_for_tests(path);
    assert!(trust
        .require_trusted_git(sibling.to_str().unwrap())
        .is_err());
    let state = GitServiceState::default();
    let generation = state.0.begin(path).unwrap();
    let env = detect_trusted_environment(&trust, path, || {
        let env = detect_environment(&target);
        assert!(state.0.finish(path, generation, &env, None)?);
        Ok(env)
    })
    .unwrap();
    let GitEnvironment::Ready { root, .. } = env else {
        panic!("fixture must be detected as a repository");
    };
    tauri::async_runtime::block_on(with_requested_repo_blocking(
        &state,
        &trust,
        root.clone(),
        |root| stage(root, &["shared.txt".into()]),
    ))
    .unwrap();
    assert_eq!(
        index_entries(&sibling),
        sibling_index,
        "suffix {suffix:?}: detected {root:?} must not stage into the sibling"
    );
    assert_eq!(
        Path::new(&root).canonicalize().unwrap(),
        intended.canonicalize().unwrap()
    );
    #[cfg(unix)]
    assert_eq!(root, intended.canonicalize().unwrap().to_str().unwrap());
    let intended_status = status_of(&intended, None).unwrap();
    assert_eq!(intended_status.parsed.staged.len(), 1);
    assert_eq!(intended_status.parsed.staged[0].path, "shared.txt");
    assert_eq!(
        std::fs::read_to_string(intended.join("shared.txt")).unwrap(),
        "intended contents"
    );
    assert_eq!(
        std::fs::read_to_string(sibling.join("shared.txt")).unwrap(),
        "sibling contents"
    );
    assert_eq!(
        std::fs::read_to_string(sibling.join("canary.txt")).unwrap(),
        "already staged"
    );
    assert!(trust
        .require_trusted_git(sibling.to_str().unwrap())
        .is_err());
    trust.0.revoke(path).unwrap();
    let denied = tauri::async_runtime::block_on(with_requested_repo_blocking(
        &state,
        &trust,
        root,
        |_| -> Result<(), String> { panic!("revoked trust must reject before operation") },
    ));
    assert!(denied.unwrap_err().contains("untrustedWorkspace"));
}

#[test]
fn ordinary_repository_identity() {
    check_root_identity("", false);
}

#[test]
fn nested_workspace_detects_parent_repository() {
    check_root_identity("", true);
}

#[cfg(unix)]
macro_rules! suffix_case {
    ($name:ident, $suffix:expr) => {
        #[test]
        fn $name() {
            check_root_identity($suffix, false);
        }
    };
}

#[cfg(unix)]
suffix_case!(trailing_space_preserves_repository_identity, " ");
#[cfg(unix)]
suffix_case!(trailing_tab_preserves_repository_identity, "\t");
#[cfg(unix)]
suffix_case!(trailing_lf_preserves_repository_identity, "\n");
#[cfg(unix)]
suffix_case!(trailing_cr_preserves_repository_identity, "\r");
#[cfg(unix)]
suffix_case!(trailing_multiple_lf_preserves_repository_identity, "\n\n");
#[cfg(unix)]
suffix_case!(
    trailing_unicode_space_preserves_repository_identity,
    "\u{3000}"
);
#[cfg(unix)]
suffix_case!(
    trailing_mixed_whitespace_preserves_repository_identity,
    " \t\r\n\n"
);
