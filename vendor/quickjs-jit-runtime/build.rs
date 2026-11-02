use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(rquickjs_memory_sanitizer)");
    println!("cargo:rustc-check-cfg=cfg(rquickjs_sanitizer)");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_SANITIZE");
    println!("cargo:rerun-if-env-changed=RUSTFLAGS");
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    let memory_sanitizer = env::var("CARGO_CFG_SANITIZE")
        .is_ok_and(|value| value.split(',').any(|value| value == "memory"))
        || env::var("RUSTFLAGS").is_ok_and(|value| value.contains("sanitizer=memory"))
        || env::var("CARGO_ENCODED_RUSTFLAGS")
            .is_ok_and(|value| value.contains("sanitizer=memory"));
    if memory_sanitizer {
        println!("cargo:rustc-cfg=rquickjs_memory_sanitizer");
    }
    let any_sanitizer = env::var("CARGO_CFG_SANITIZE").is_ok_and(|value| !value.is_empty())
        || env::var("RUSTFLAGS").is_ok_and(|value| value.contains("sanitizer="))
        || env::var("CARGO_ENCODED_RUSTFLAGS").is_ok_and(|value| value.contains("sanitizer="));
    if any_sanitizer {
        println!("cargo:rustc-cfg=rquickjs_sanitizer");
    }

    const EXPECTED_COUNT: usize = 252;
    const EXPECTED_FINGERPRINT: u64 = 0x05d5_c086_7521_c077;
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let opcodes = rquickjs_sys::QJSJIT_GENERATED_OPCODES;
    let audit = include_str!("src/bytecode/policy_audit.rs");
    let audited: Vec<(usize, &str)> = audit
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let id = line.strip_prefix("AuditedOpcodePolicy { id: ")?;
            let (id, rest) = id.split_once(',')?;
            let name = rest.split("name: \"").nth(1)?.split('"').next()?;
            Some((id.parse().expect("numeric audited opcode id"), name))
        })
        .collect();
    assert_eq!(opcodes.len(), rquickjs_sys::QJSJIT_GENERATED_OPCODE_COUNT);
    assert_eq!(
        opcodes.len(),
        EXPECTED_COUNT,
        "QuickJS opcode count changed; audit and regenerate the closed Tier 1 policy"
    );
    assert_eq!(
        audited.len(),
        EXPECTED_COUNT,
        "every opcode ID needs an explicit audited policy row"
    );
    for (expected, (id, name)) in audited.iter().enumerate() {
        assert_eq!(
            *id, expected,
            "audited opcode IDs must be dense and ordered"
        );
        assert_eq!(
            *name, opcodes[expected].name,
            "audited opcode name/ID drifted from QuickJS"
        );
    }
    assert_eq!(
        rquickjs_sys::QJSJIT_GENERATED_OPCODE_FINGERPRINT,
        EXPECTED_FINGERPRINT,
        "QuickJS opcode metadata changed; audit and regenerate the closed Tier 1 policy"
    );
    let generated = format!(
        "pub const GENERATED_OPCODE_COUNT: usize = {};\n\
         pub const GENERATED_OPCODE_FINGERPRINT: u64 = 0x{:016x};\n",
        opcodes.len(),
        rquickjs_sys::QJSJIT_GENERATED_OPCODE_FINGERPRINT
    );
    for (expected_id, opcode) in opcodes.iter().enumerate() {
        assert_eq!(
            usize::from(opcode.opcode),
            expected_id,
            "opcode IDs are not dense"
        );
    }
    fs::write(out.join("tier1-opcodes.rs"), generated).expect("write Tier 1 opcode identities");
}
