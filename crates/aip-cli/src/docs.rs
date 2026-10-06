//! Parses the ```aip examples embedded in design documents so that the
//! documentation can never drift from the grammar the compiler implements.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

struct Block {
    file: PathBuf,
    line: usize,
    body: String,
}

fn collect(path: &Path, out: &mut Vec<Block>) -> Result<()> {
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            collect(&p, out)?;
        }
        return Ok(());
    }
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return Ok(());
    }
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines().enumerate();
    while let Some((i, line)) = lines.next() {
        if line.trim_start() == "```aip" {
            let mut body = String::new();
            for (_, l) in lines.by_ref() {
                if l.trim_start() == "```" {
                    break;
                }
                body.push_str(l);
                body.push('\n');
            }
            out.push(Block { file: path.to_path_buf(), line: i + 2, body });
        }
    }
    Ok(())
}

/// Blocks with `...` placeholders are illustrative fragments, not programs.
fn is_fragment(body: &str) -> bool {
    body.contains("...") && !body.contains("...input") && !body.contains("...expr")
}

pub fn check_docs(paths: &[PathBuf]) -> Result<ExitCode> {
    let mut blocks = Vec::new();
    for p in paths {
        collect(p, &mut blocks)?;
    }
    let (mut ok, mut failed, mut skipped) = (0, 0, 0);
    for b in &blocks {
        if is_fragment(&b.body) {
            skipped += 1;
            continue;
        }
        match aip_syntax::parse_snippet(&b.body) {
            Ok(_) => ok += 1,
            Err(d) => {
                failed += 1;
                let line = b.line + d.span.line as usize - 1;
                println!("{}:{}: {} {}", b.file.display(), line, d.code, d.message);
                if let Some(h) = &d.help {
                    println!("    help: {h}");
                }
                let src_line = b.body.lines().nth(d.span.line as usize - 1).unwrap_or("");
                println!("    | {}", src_line.trim_end());
            }
        }
    }
    println!("\n{} blocks: {ok} parsed, {failed} failed, {skipped} skipped (fragments with '...')", blocks.len());
    Ok(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
