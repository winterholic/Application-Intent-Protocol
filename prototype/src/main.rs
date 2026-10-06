use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};
use spike_v11_dev_checks::Checked;
use spike_v2_read::id_wire::IdWire;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
};

mod runtime;

#[derive(Parser)]
#[command(name = "aip-prototype", about = "AIP 호출자 표현 프로토타입")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "새 전용 schema만 초기화한다. 기존 데이터는 변경하지 않는다")]
    Init {
        file: PathBuf,
        #[arg(long)]
        schema: String,
        #[arg(long)]
        db_url: String,
    },
    #[command(about = "초기화한 schema에서 loopback 개발 서버를 실행한다")]
    Serve(runtime::Serve),
    #[command(about = "정의를 검사한다. 개발 조언은 선택 사항이다")]
    Check {
        file: PathBuf,
        #[arg(long)]
        dev_checks: bool,
    },
    #[command(about = "공개 읽기·쓰기 계약을 생성한다")]
    Gen {
        file: PathBuf,
        #[arg(long, value_enum)]
        wire: Wire,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        sdk_import: String,
        #[arg(long)]
        dev_checks: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Wire {
    Safe,
    Decimal,
}

impl Wire {
    fn id_wire(self) -> IdWire {
        match self {
            Self::Safe => IdWire::SafeNumber,
            Self::Decimal => IdWire::DecimalString,
        }
    }
}

fn error(code: &str, msg: &str) -> Value {
    json!({"ok":false,"code":code,"msg":msg})
}

fn report(output: &Value) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{output}")?;
    stdout.flush()
}

fn report_error(output: &Value) {
    let _ = writeln!(std::io::stderr().lock(), "{output}");
}

fn load(path: &Path, dev_checks: bool) -> Result<Checked, Value> {
    let form = path.to_str().and_then(spike_v1_fixture::form_of).ok_or_else(|| error("UNSUPPORTED_FORM", "지원하는 AIP 작성 형식이 아님"))?;
    let source = fs::read_to_string(path).map_err(|_| error("SOURCE_IO", "정의 파일을 읽을 수 없음"))?;
    spike_v11_dev_checks::load_checked(&source,form,&json!({"devChecks":dev_checks})).map_err(|diagnostics| {
        json!({"ok":false,"code":"INVALID_DEFINITION","diagnostics":diagnostics.iter().map(|d|json!({"code":d.code,"msg":d.msg,"line":d.span.line,"col":d.span.col})).collect::<Vec<_>>()})
    })
}

fn source_conflict(source: &Path, out: &Path) -> Result<bool, Value> {
    let source_path = source.canonicalize().map_err(|_| error("SOURCE_IO", "정의 경로를 확인할 수 없음"))?;
    let out_path = match out.canonicalize() {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(error("OUTPUT_IO", "출력 경로를 확인할 수 없음")),
    };
    if source_path == out_path {
        return Ok(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let a = fs::metadata(source).map_err(|_| error("SOURCE_IO", "정의 파일을 확인할 수 없음"))?;
        let b = fs::metadata(out).map_err(|_| error("OUTPUT_IO", "출력 파일을 확인할 수 없음"))?;
        if a.dev() == b.dev() && a.ino() == b.ino() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn write_binding(path: &Path, content: &str) -> Result<(), Value> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let temporary = parent.join(format!(".aip-gen-{}.tmp", std::process::id()));
    let mut file =
        fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|_| error("OUTPUT_IO", "출력 임시 파일을 생성할 수 없음"))?;
    // Keep the last usable binding intact until the replacement is fully written.
    let result = file.write_all(content.as_bytes()).and_then(|_| file.sync_all()).and_then(|_| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|_| error("OUTPUT_IO", "계약 파일을 교체할 수 없음"))
}

async fn run(cli: Cli) -> Result<Value, Value> {
    match cli.command {
        Command::Init { file, schema, db_url } => runtime::init(&file, &schema, &db_url).await,
        Command::Serve(args) => runtime::serve(args).await,
        Command::Check { file, dev_checks } => {
            let checked = load(&file, dev_checks)?;
            Ok(
                json!({"ok":true,"executionDigest":spike_v1_fixture::digest(&checked.output.execution),"advisories":checked.advisories.iter().map(|a|json!({"code":a.code,"anchor":a.anchor,"msg":a.msg})).collect::<Vec<_>>()}),
            )
        }
        Command::Gen { file, dev_checks, wire, out, sdk_import } => {
            let checked = load(&file, dev_checks)?;
            if source_conflict(&file, &out)? {
                return Err(error("OUTPUT_CONFLICT", "계약 출력으로 원본 정의를 덮어쓸 수 없음"));
            }
            if sdk_import.trim().is_empty() {
                return Err(error("BAD_SDK_IMPORT", "SDK import 경로가 비었음"));
            }
            let wire = wire.id_wire();
            let facts = &checked.output.execution;
            let binding = spike_v5_sdk::contract_module_with_all_extensions(facts, &sdk_import, wire);
            write_binding(&out, &binding)?;
            Ok(json!({"ok":true,"fingerprint":spike_v5_sdk::contract_fingerprint_with_all_extensions(facts,wire),"idWire":wire.label()}))
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(output) => {
            match report(&output) {
                Ok(()) => ExitCode::SUCCESS,
                // 소비자가 결과 읽기를 취소해도 이미 수행한 효과를 실패로 바꾸지 않는다.
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
                Err(_) => {
                    report_error(&error("OUTPUT_IO", "명령 결과 출력 실패"));
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            report_error(&error);
            ExitCode::FAILURE
        }
    }
}
