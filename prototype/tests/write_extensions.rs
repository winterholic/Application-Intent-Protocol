static NEXT_DIR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
use std::{fs, path::PathBuf, process::Command};
const APP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../spikes/spike-v6-transport/tests/fixtures/write-extension.aip");
struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn directory() -> OwnedDir {
    let path = std::env::temp_dir().join(format!(
        "aip-write-cli-{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        NEXT_DIR.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    OwnedDir(path)
}

#[test]
fn generated_write_contract_compiles_real_sdk_usage_and_rejects_invalid_calls() {
    let dir = directory();
    let sdk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("sdk/index.ts");
    let tsc = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../spikes/spike-0-ts/node_modules/.bin/tsc");
    for wire in ["safe", "decimal"] {
        let contract = dir.0.join("contract.ts");
        let generated = Command::new(env!("CARGO_BIN_EXE_aip-prototype"))
            .args(["gen", APP, "--wire", wire, "--out", contract.to_str().unwrap(), "--sdk-import", sdk.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(generated.status.success(), "gen: {}", String::from_utf8_lossy(&generated.stderr));
        let id = if wire == "safe" { "11" } else { "'11'" };
        let wrong = if wire == "safe" { "'11'" } else { "11" };
        let output_type = if wire == "safe" { "number" } else { "string" };
        let sdk_import = serde_json::to_string(sdk.to_str().unwrap()).unwrap();
        let source=format!("import {{connect}} from {sdk_import};\nimport {{contract}} from './contract.ts';\nconst client=connect('http://localhost:1234',null,contract);\nasync function use(){{\nconst result=await client.writeExtension('Event.run',{{id:{id}}},{{key:'stable'}});\nif(result.ok){{const id:{output_type}=result.output.id;const count:number=result.output.count;void id;void count;\n// @ts-expect-error readonly\nresult.output.count=2;\n}}\n// @ts-expect-error unknown-name\nawait client.writeExtension('Event.missing',{{id:{id}}});\n// @ts-expect-error wrong-wire\nawait client.writeExtension('Event.run',{{id:{wrong}}});\n}}\nvoid use;\n");
        let consumer = dir.0.join("consumer.mts");
        fs::write(&consumer, &source).unwrap();
        let check = |path: &std::path::Path| {
            Command::new(&tsc)
                .args([
                    "--noEmit",
                    "--strict",
                    "--target",
                    "esnext",
                    "--module",
                    "nodenext",
                    "--moduleResolution",
                    "nodenext",
                    "--allowImportingTsExtensions",
                    path.to_str().unwrap(),
                ])
                .output()
                .unwrap()
        };
        let positive = check(&consumer);
        assert!(positive.status.success(), "{wire} valid generated SDK usage: {}", String::from_utf8_lossy(&positive.stdout));
        for name in ["readonly", "unknown-name", "wrong-wire"] {
            let marker = format!("// @ts-expect-error {name}\n");
            assert_eq!(source.matches(&marker).count(), 1);
            fs::write(&consumer, source.replace(&marker, "")).unwrap();
            let negative = check(&consumer);
            assert!(!negative.status.success(), "{wire} {name} marker removal must fail");
        }
    }
}

#[test]
fn write_worker_opt_in_checks_flags_and_implementation_before_database() {
    let dir = directory();
    fs::write(dir.0.join("write.mjs"), "export async function run(){return {id:11,count:1};}\n").unwrap();
    let base = [
        "serve",
        APP,
        "--schema",
        "aip_write_options",
        "--db-url",
        "host=127.0.0.1 port=1 dbname=postgres connect_timeout=1",
        "--listen",
        "127.0.0.1:0",
        "--wire",
        "safe",
        "--enable-write-extensions",
    ];
    let missing = Command::new(env!("CARGO_BIN_EXE_aip-prototype")).args(base).output().unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--worker-dir"), "WRITE requires existing paired worker options");
    let valid = Command::new(env!("CARGO_BIN_EXE_aip-prototype"))
        .args(base)
        .args(["--worker-dir", dir.0.to_str().unwrap(), "--worker-lang", "node"])
        .output()
        .unwrap();
    let error: serde_json::Value = serde_json::from_slice(&valid.stderr).unwrap();
    assert_eq!(error["code"], "DB_CONNECT", "recognized opt-in and valid module reach failing local DB");
    fs::remove_file(dir.0.join("write.mjs")).unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_aip-prototype"))
        .args(base)
        .args(["--worker-dir", dir.0.to_str().unwrap(), "--worker-lang", "node"])
        .output()
        .unwrap();
    let error: serde_json::Value = serde_json::from_slice(&invalid.stderr).unwrap();
    assert_eq!(error["code"], "WORKER_CONFIG", "missing WRITE implementation precedes DB");
}
