use std::process::Command;

#[test]
fn skill_prints_the_window_manual() {
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-app")).arg("--skill").output().unwrap();
    assert!(output.status.success());
    let skill = String::from_utf8(output.stdout).unwrap();
    assert_eq!(skill, include_str!("../src/SKILL.md"));
    assert!(skill.lines().count() < 500, "skill must stay under 500 lines");
    assert!(skill.starts_with("# herdr-app:"));
    assert!(skill.contains("hc --skill"));
}
