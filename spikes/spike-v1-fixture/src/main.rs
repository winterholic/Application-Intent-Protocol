use serde_json::json;
use spike_v1_fixture::{digest, form_of, load_str};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (cmd, path) = match (args.get(1).map(String::as_str), args.get(2)) {
        (Some(c @ ("facts" | "check")), Some(p)) if args.len() == 3 => (c, p.as_str()),
        _ => {
            eprintln!("usage: spike-v1-fixture <facts|check> <file(.aip|.e.ts|.e.py|.h.ts|.h.py)>");
            return ExitCode::from(2);
        }
    };
    let Some(form) = form_of(path) else {
        eprintln!("알 수 없는 파일 형식: {path}");
        return ExitCode::from(2);
    };
    let src = std::fs::read_to_string(path).expect("read");
    match load_str(&src, form) {
        Ok(o) => {
            let ed = digest(&o.execution);
            let md = digest(&o.metadata);
            match cmd {
                "facts" => println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "executionDigest": ed, "metadataDigest": md,
                        "execution": o.execution, "metadata": o.metadata, "spans": o.spans,
                    }))
                    .unwrap()
                ),
                _ => println!("ok {form:?} exec={} meta={}", &ed[..16], &md[..16]),
            }
            ExitCode::SUCCESS
        }
        Err(ds) => {
            for d in ds {
                println!("{path}:{d}");
            }
            ExitCode::from(1)
        }
    }
}
