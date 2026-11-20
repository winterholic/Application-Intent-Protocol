#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `aip --help` tells the 1.0 commands from the PoC ones, and no PoC command is removed (X12).
use std::process::Command;

fn aip(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aip")).args(args).output().expect("run aip")
}

/// `(command, description)` of every line of the `Commands:` section.
fn commands(help: &str) -> Vec<(String, String)> {
    help.lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| {
            let (name, rest) = line.trim_start().split_once(' ')?;
            Some((name.to_string(), rest.trim().to_string()))
        })
        .collect()
}

#[test]
fn the_root_help_marks_the_1_0_commands_and_the_poc_commands_apart() {
    let out = aip(&["--help"]);
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    let listed = commands(&help);
    let names: Vec<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
    for expected in [
        "service",
        "explain-code",
        "diagnostics",
        "parse",
        "check",
        "ddl",
        "ir",
        "core",
        "explain",
        "migrate",
        "rekey",
        "run",
        "diff",
        "token",
        "contract",
        "gen-ts",
        "check-docs",
    ] {
        assert!(names.contains(&expected), "`aip {expected}` is no longer listed: {names:?}");
    }
    let official = ["service", "explain-code", "diagnostics"];
    for (name, description) in &listed {
        if name == "help" {
            continue;
        }
        let marker = if official.contains(&name.as_str()) { "[1.0]" } else { "[PoC]" };
        assert!(description.starts_with(marker), "`{name}` must start with {marker}: {description}");
    }
    let last_official = listed.iter().rposition(|(name, _)| official.contains(&name.as_str())).unwrap();
    let first_poc = listed.iter().position(|(_, description)| description.starts_with("[PoC]")).unwrap();
    assert!(last_official < first_poc, "the 1.0 commands are listed before the PoC commands: {names:?}");
    assert!(help.contains("Commands marked [1.0]") && help.contains("aip service --help"), "{help}");
}

#[test]
fn a_poc_command_still_answers_its_own_help() {
    for command in ["run", "token", "migrate", "gen-ts", "check"] {
        let out = aip(&[command, "--help"]);
        assert!(out.status.success(), "`aip {command} --help`: {}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains("[PoC]"), "`aip {command} --help` does not say it is a PoC command");
    }
}
