//! Without the optional `poc-runtime` package, the PoC runtime commands refuse with a clear error before touching anything.
#![cfg(not(feature = "poc-runtime"))]

use std::process::Command;

fn aip(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aip")).args(args).output().expect("run aip")
}

#[test]
fn poc_runtime_commands_name_the_missing_feature() {
    let app = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/ariari/app.aip");
    for args in [vec!["run", app], vec!["migrate", app], vec!["rekey", app], vec!["token", "7"], vec!["diff", app, "--against", "deployed"]] {
        let out = aip(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("RUNTIME_FEATURE_DISABLED") && stderr.contains("--features poc-runtime"), "{args:?}: {stderr}");
    }
}

#[test]
fn compiler_commands_and_diff_against_a_file_work_without_the_package() {
    let app = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/ariari/app.aip");
    assert!(aip(&["check", app]).status.success());
    let core = aip(&["core", app]);
    assert!(core.status.success());
    let file = std::env::temp_dir().join(format!("aip-core-{}.json", std::process::id()));
    std::fs::write(&file, &core.stdout).expect("write core");
    let diff = aip(&["diff", app, "--against", file.to_str().expect("path")]);
    std::fs::remove_file(&file).ok();
    assert!(diff.status.success(), "{}", String::from_utf8_lossy(&diff.stderr));
}
