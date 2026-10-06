use spike_v4_worker::{Isolation, Lang, Worker, WorkerLimits};
use std::fs;
use std::io;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[tokio::test]
async fn missing_program_helper() {
    let Ok(probe) = std::env::var("AIP_WORKER_START_PROBE") else {
        return;
    };
    let lang = match probe.as_str() {
        "node" => Lang::Node,
        "python" => Lang::Python,
        other => panic!("unknown worker start probe: {other}"),
    };
    let missing_ext_dir = std::env::current_dir().expect("probe working directory").join("missing-extension");
    let ext_dir = missing_ext_dir.to_string_lossy();
    match Worker::try_start_with(lang, &ext_dir, Isolation::None, WorkerLimits::default()).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("startup probe: {probe} returned NotFound");
        }
        Err(error) => panic!("startup probe: {probe} returned {:?}, expected NotFound", error.kind()),
        Ok(worker) => {
            worker.stop().await;
            panic!("startup probe: {probe} unexpectedly started without PATH executables");
        }
    }
}

fn child_probe(executable: &std::path::Path, probe: &str, work_dir: &std::path::Path) -> Result<(bool, String, String), String> {
    fs::create_dir(work_dir).map_err(|error| format!("temp directory creation: {:?}", error.kind()))?;
    let result = (|| {
        let mut child = Command::new(executable)
            .args(["--exact", "missing_program_helper", "--nocapture"])
            .current_dir(work_dir)
            .env_clear()
            .env("AIP_WORKER_START_PROBE", probe)
            .env("PATH", "")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("test helper spawn: {:?}", error.kind()))?;

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut timed_out = false;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                Ok(None) => {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("test helper wait: {:?}", error.kind()));
                }
            }
        }
        let output = child.wait_with_output().map_err(|error| format!("test helper output: {:?}", error.kind()))?;
        if timed_out {
            return Err("test helper exceeded five seconds and was killed".into());
        }
        Ok((output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned()))
    })();
    let cleanup = fs::remove_dir_all(work_dir).map_err(|error| format!("temp directory cleanup: {:?}", error.kind()));
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup_error)) => Err(format!("{error}; {cleanup_error}")),
    }
}

#[test]
fn startup_spawn_failures_are_io_not_found_for_node_and_python() {
    let executable = std::env::current_exe().expect("current test executable");
    let mut failures = Vec::new();
    let mut output_table = Vec::new();

    for probe in ["node", "python"] {
        let work_dir = std::env::temp_dir().join(format!("aip-startup-probe-{}-{probe}", std::process::id()));
        match child_probe(&executable, probe, &work_dir) {
            Ok((success, stdout, stderr)) => {
                let marker = format!("startup probe: {probe} returned NotFound");
                output_table.push(format!("{probe} | exit={success} | stdout={stdout:?} | stderr={stderr:?}"));
                if !success || !stdout.contains(&marker) || !stderr.is_empty() {
                    failures.push(format!("{probe}: 자식 실행 결과 또는 stdout/stderr가 예상과 다름"));
                }
            }
            Err(error) => failures.push(format!("{probe}: {error}")),
        }
    }

    eprintln!("{}", output_table.join("\n"));
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
