use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod docs;
mod explain;

#[derive(Parser)]
#[command(name = "aip", version, about = "AIP compiler and runtime")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the caller-contract product service, with external access-token authentication
    Service {
        #[command(subcommand)]
        command: aip_service::Command,
    },
    /// Parse a definition file and print the AST as JSON
    Parse { file: PathBuf },
    /// Parse and type-check a definition file
    Check {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Print the PostgreSQL DDL for a definition
    Ddl { file: PathBuf },
    /// Print the compiled execution plans (IR) as JSON
    Ir { file: PathBuf },
    /// Print the canonical semantic IR (Core IR) as JSON, or its digest
    Core {
        file: PathBuf,
        #[arg(long)]
        digest: bool,
    },
    /// Explain one intent: its steps and exact SQL
    Explain { file: PathBuf, intent: String },
    /// Explain a diagnostic or error code (for example AIP-E103 or AIP.CONFLICT.UNIQUE): meaning, fix, HTTP status
    ExplainCode {
        code: String,
        #[arg(long)]
        json: bool,
    },
    /// List every diagnostic and error code of the registry (`--markdown` is the text of spec/diagnostics.md, `--json` the entries)
    Diagnostics {
        #[arg(long)]
        markdown: bool,
        #[arg(long)]
        json: bool,
    },
    /// Apply the schema to DATABASE_URL: the first time everything is created, later the change from the deployed program is planned and applied
    /// in one transaction (`--plan` only prints it; `--reset` drops and recreates)
    Migrate {
        file: PathBuf,
        #[arg(long)]
        reset: bool,
        /// Print the steps and what stops them, change nothing
        #[arg(long)]
        plan: bool,
        /// With --plan: the report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Re-encrypt every encrypted value that is not under the current key (the first of AIP_ENCRYPTION_KEYS); safe to run again
    Rekey { file: PathBuf },
    /// Run the HTTP runtime
    Run {
        file: PathBuf,
        #[arg(long, default_value_t = 4000)]
        port: u16,
        /// Drop and recreate the schema before starting (development)
        #[arg(long)]
        reset: bool,
        /// Accept `x-aip-actor` headers (development only)
        #[arg(long)]
        dev_auth: bool,
        /// Let outbound webhooks call private, loopback and link-local addresses (development and tests only)
        #[arg(long)]
        allow_private_webhook_targets: bool,
    },
    /// Compare the contract of a definition with a deployed version: breaking changes, behavior changes, compatible ones (exit 1 when something breaks)
    Diff {
        file: PathBuf,
        /// A Core IR JSON file (`aip core`), or `deployed` for the program recorded in the database of DATABASE_URL
        #[arg(long)]
        against: String,
        #[arg(long)]
        json: bool,
    },
    /// Issue a signed actor token (uses AIP_SECRET)
    Token {
        actor: String,
        #[arg(long, default_value_t = 86400)]
        ttl: i64,
    },
    /// Print the machine-readable contract (`--operator`: the server-side setup instead: webhook signing, job file retention)
    Contract {
        file: PathBuf,
        #[arg(long)]
        operator: bool,
    },
    /// Generate a typed TypeScript client from the contract
    GenTs { file: PathBuf, out: PathBuf },
    /// Parse every ```aip block in markdown files under the given paths
    CheckDocs {
        #[arg(default_values = ["docs", "spec"])]
        paths: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Service { command } => Ok(aip_service::execute(command)),
        Cmd::Parse { file } => {
            let src = read(&file)?;
            match aip_syntax::parse_file(&src) {
                Ok(ast) => {
                    println!("{}", serde_json::to_string_pretty(&ast)?);
                    Ok(ExitCode::SUCCESS)
                }
                Err(d) => {
                    eprintln!("{}", d.render(&file.display().to_string()));
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        Cmd::CheckDocs { paths } => docs::check_docs(&paths),
        Cmd::GenTs { file, out } => {
            let Some((core, _)) = compile_full(&file)? else { return Ok(ExitCode::FAILURE) };
            let ts = aip_contract::gen_ts(&core);
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&out, ts)?;
            println!("wrote {}", out.display());
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Contract { file, operator } => {
            let Some((core, _)) = compile_full(&file)? else { return Ok(ExitCode::FAILURE) };
            let doc = if operator { aip_contract::operator(&core) } else { aip_contract::describe(&core) };
            println!("{}", serde_json::to_string_pretty(&doc)?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Diff { file, against, json } => {
            let Some((core, _)) = lower_core_with_map(&file)? else { return Ok(ExitCode::FAILURE) };
            let old: aip_ir::Program = if against == "deployed" {
                let url = database_url()?;
                let rt = tokio::runtime::Runtime::new()?;
                let pool = aip_runtime::pool(&url, 1)?;
                rt.block_on(aip_runtime::deployed_core(&pool))?.ok_or_else(|| {
                    anyhow::anyhow!("the database of DATABASE_URL has no deployment record (_aip_deployment); nothing to compare with")
                })?
            } else {
                serde_json::from_str(&read(Path::new(&against))?).with_context(|| format!("{against} is not a Core IR document (`aip core`)"))?
            };
            let findings = aip_contract::compat::compare(&old, &core);
            let breaking = aip_contract::compat::breaks(&findings);
            if json {
                println!("{}", serde_json::to_string_pretty(&findings)?);
            } else {
                for f in &findings {
                    println!("{:<10} {:<26} {}: {}", format!("{:?}", f.level).to_lowercase(), f.rule, f.subject, f.message);
                }
                let count = |l: aip_contract::compat::Level| findings.iter().filter(|f| f.level == l).count();
                println!(
                    "{} breaking, {} warning(s), {} compatible",
                    count(aip_contract::compat::Level::Breaking),
                    count(aip_contract::compat::Level::Warning),
                    count(aip_contract::compat::Level::Compatible)
                );
            }
            Ok(if breaking { ExitCode::FAILURE } else { ExitCode::SUCCESS })
        }
        Cmd::Token { actor, ttl } => {
            let secret = std::env::var("AIP_SECRET").map_err(|_| anyhow::anyhow!("AIP_SECRET is not set"))?;
            println!("{}", aip_runtime::auth::issue(secret.as_bytes(), &actor, ttl));
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Migrate { file, reset, plan, json } => {
            let Some((core, program)) = compile_full(&file)? else { return Ok(ExitCode::FAILURE) };
            let url = database_url()?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let pool = aip_runtime::pool(&url, 2)?;
                let evolve = |old: &aip_ir::Program| aip_pg::evolve::plan(old, &core, &program.ddl);
                let deploy = aip_runtime::Deployment { core: &core, ddl_version: aip_pg::evolve::DDL_VERSION, evolve: &evolve };
                if plan {
                    anyhow::ensure!(!reset, "--plan changes nothing, so it cannot be combined with --reset");
                    let report = aip_runtime::plan(&pool, &program, &deploy).await?;
                    return print_plan(&report, json);
                }
                let r = aip_runtime::migrate_deployed(&pool, &program, &deploy, reset).await?;
                println!("migrated ({:?}): {} steps, {} data migration(s)", r.kind, r.applied, r.migrations.len());
                for n in &r.notes {
                    println!("note: {n}");
                }
                Ok(ExitCode::SUCCESS)
            })
        }
        Cmd::Rekey { file } => {
            let Some((_, program)) = compile_full(&file)? else { return Ok(ExitCode::FAILURE) };
            let keys = aip_runtime::crypto::load(&program, std::env::var(aip_runtime::crypto::ENV_KEYS).ok().as_deref())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let Some(keys) = keys else {
                println!("nothing to do: the program has no encrypted fields");
                return Ok(ExitCode::SUCCESS);
            };
            let url = database_url()?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let pool = aip_runtime::pool(&url, 2)?;
                println!("current key: {} (can read with: {})", keys.current(), keys.ids().join(", "));
                let mut bad = false;
                for r in aip_runtime::rekey::rekey(&pool, &program, &keys).await? {
                    println!(
                        "{} in {}: {} re-encrypted, {} already current, {} changed meanwhile, {} unreadable",
                        r.field,
                        r.table,
                        r.rekeyed,
                        r.already_current,
                        r.raced,
                        r.failed.len()
                    );
                    for id in &r.failed {
                        println!("  unreadable (key unknown, or the value was changed or moved): {id}");
                    }
                    bad |= !r.failed.is_empty() || r.raced > 0;
                }
                Ok(if bad { ExitCode::FAILURE } else { ExitCode::SUCCESS })
            })
        }
        Cmd::Run { file, port, reset, dev_auth, allow_private_webhook_targets } => {
            tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
            let Some((core, program)) = compile_full(&file)? else { return Ok(ExitCode::FAILURE) };
            // the runtime serves the contract as computed from Core IR; it never sees the IR itself
            let contract = aip_contract::describe(&core);
            let url = database_url()?;
            // before the database is touched: a program with encrypted fields does not start without usable keys
            let keys = aip_runtime::crypto::load(&program, std::env::var(aip_runtime::crypto::ENV_KEYS).ok().as_deref())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let secret = match std::env::var("AIP_SECRET") {
                Ok(s) => s.into_bytes(),
                Err(_) if dev_auth => b"aip-dev-secret".to_vec(),
                Err(_) => anyhow::bail!("AIP_SECRET is not set (or pass --dev-auth for local development)"),
            };
            if dev_auth {
                eprintln!("WARNING: --dev-auth accepts x-aip-actor headers; never use it in production");
            }
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let pool = aip_runtime::pool(&url, 2)?;
                let evolve = |old: &aip_ir::Program| aip_pg::evolve::plan(old, &core, &program.ddl);
                let deploy = aip_runtime::Deployment { core: &core, ddl_version: aip_pg::evolve::DDL_VERSION, evolve: &evolve };
                let r = aip_runtime::migrate_deployed(&pool, &program, &deploy, reset).await?;
                if r.applied > 0 {
                    eprintln!("schema: {:?}, {} steps", r.kind, r.applied);
                }
                for n in &r.notes {
                    eprintln!("note: {n}");
                }
                aip_runtime::run(
                    program,
                    aip_runtime::Options {
                        database_url: url,
                        port,
                        secret,
                        dev_auth,
                        trusted_proxies: std::env::var("AIP_TRUSTED_PROXIES").is_ok(),
                        object_dir: std::env::var("AIP_OBJECT_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".aip/objects")),
                        workers: true,
                        allow_private_webhook_targets,
                        contract,
                        keys,
                    },
                )
                .await?;
                Ok(ExitCode::SUCCESS)
            })
        }
        Cmd::Ddl { file } => {
            let Some(program) = compile(&file)? else { return Ok(ExitCode::FAILURE) };
            for s in &program.ddl {
                println!("{s};\n");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Ir { file } => {
            let Some(program) = compile(&file)? else { return Ok(ExitCode::FAILURE) };
            println!("{}", serde_json::to_string_pretty(&program)?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Core { file, digest } => {
            let Some((program, _)) = lower_core_with_map(&file)? else { return Ok(ExitCode::FAILURE) };
            if digest {
                println!("{}", aip_ir::digest(&program));
            } else {
                println!("{}", serde_json::to_string_pretty(&program)?);
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Explain { file, intent } => {
            let Some(program) = compile(&file)? else { return Ok(ExitCode::FAILURE) };
            match program.intents.get(&intent) {
                Some(i) => {
                    print!("{}", explain::explain(i));
                    if let Some(j) = program.jobs.iter().find(|j| j.name == intent) {
                        print!("\n{}", explain::explain_job(j));
                    }
                    Ok(ExitCode::SUCCESS)
                }
                None => {
                    eprintln!("no intent named {intent}");
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        Cmd::ExplainCode { code, json } => match aip_ir::codes::lookup(&code.to_uppercase()) {
            Some(c) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(c)?);
                } else {
                    print!("{}", aip_ir::codes::render_text(c));
                }
                Ok(ExitCode::SUCCESS)
            }
            None => {
                eprintln!("unknown code {code}; `aip diagnostics` lists the registered codes");
                Ok(ExitCode::FAILURE)
            }
        },
        Cmd::Diagnostics { markdown, json } => {
            if markdown {
                print!("{}", aip_ir::codes::render_markdown());
            } else if json {
                println!("{}", serde_json::to_string_pretty(aip_ir::codes::all())?);
            } else {
                for c in aip_ir::codes::all() {
                    println!("{:<36} {:<8} {}", c.code, if c.severity == aip_ir::codes::Severity::Warning { "warning" } else { "error" }, c.title);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Check { file, json } => {
            let name = file.display().to_string();
            let checked = aip_sema::pipeline::check_source(&read(&file)?);
            if !checked.parsed {
                // a source that does not parse has exactly one diagnostic and no summary
                if json {
                    println!("{}", serde_json::to_string_pretty(&checked.diagnostics)?);
                } else {
                    eprintln!("{}", checked.diagnostics[0].render(&name));
                }
                return Ok(ExitCode::FAILURE);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&checked.diagnostics)?);
            } else {
                for d in &checked.diagnostics {
                    println!("{}", d.render(&name));
                }
                let errors = checked.diagnostics.iter().filter(|d| d.severity == aip_syntax::Severity::Error).count();
                println!("{errors} error(s), {} warning(s)", checked.diagnostics.len() - errors);
            }
            Ok(if checked.has_errors() { ExitCode::FAILURE } else { ExitCode::SUCCESS })
        }
    }
}

/// Prints what `aip migrate --plan` found; the exit code is 1 when applying would be refused.
fn print_plan(report: &aip_runtime::PlanReport, json: bool) -> Result<ExitCode> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(if report.blocked() { ExitCode::FAILURE } else { ExitCode::SUCCESS });
    }
    println!("plan: {:?}, {} steps", report.kind, report.plan.steps.len());
    for (i, s) in report.plan.steps.iter().enumerate() {
        let mut lines = s.sql.lines();
        let first = lines.next().unwrap_or("(check only)");
        let first: String = if first.chars().count() > 110 { first.chars().take(110).collect::<String>() + "..." } else { first.to_string() };
        let more = lines.count();
        let more = if more > 0 { format!(" (+{more} lines)") } else { String::new() };
        println!("{:>3}. [{}] {first}{more}\n       {}", i + 1, format!("{:?}", s.class).to_lowercase(), s.why);
        for c in report.checks.iter().filter(|c| c.step == i) {
            match c.violations {
                Some(0) => println!("       check ok: {}", c.what),
                Some(n) => println!("       check FAILED: {} ({n} row(s), for example {})", c.what, c.sample.join(", ")),
                None => println!("       check deferred: {} (it refers to something this change creates; it runs when applied)", c.what),
            }
        }
    }
    for n in &report.plan.notes {
        println!("note: {n}");
    }
    for r in &report.plan.rejections {
        println!("refused {}: {}\n       fix: {}", r.code, r.message, r.fix);
    }
    if !report.pending_migrations.is_empty() {
        println!("data migrations to run afterwards: {}", report.pending_migrations.join(", "));
    }
    println!("{}", if report.blocked() { "blocked: applying this program would be refused" } else { "ready: `aip migrate` applies this" });
    Ok(if report.blocked() { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Parse, check, lower to Core IR, validate, analyze and compile; prints diagnostics and returns None on errors.
pub fn compile(file: &Path) -> Result<Option<aip_plan::Program>> {
    Ok(compile_full(file)?.map(|(_, plan)| plan))
}

/// Like [`compile`], and also returns the Core IR the plan came from.
pub fn compile_full(file: &Path) -> Result<Option<(aip_ir::Program, aip_plan::Program)>> {
    let name = file.display().to_string();
    let Some((core, map)) = lower_core_with_map(file)? else { return Ok(None) };
    let compiled = aip_pg::compile(&core, &map);
    for d in &compiled.diagnostics {
        eprintln!("{}", d.render(&name));
    }
    if compiled.diagnostics.iter().any(|d| d.is_error()) {
        return Ok(None);
    }
    Ok(Some((core, compiled.program)))
}

/// The shared front half of every command that needs a program: `aip_sema::pipeline::check_source`, diagnostics
/// to stderr, `None` when any of them is an error.
fn lower_core_with_map(file: &Path) -> Result<Option<(aip_ir::Program, aip_ir::SourceMap)>> {
    let name = file.display().to_string();
    let checked = aip_sema::pipeline::check_source(&read(file)?);
    for d in &checked.diagnostics {
        eprintln!("{}", d.render(&name));
    }
    Ok(checked.into_core())
}

fn database_url() -> Result<String> {
    Ok(std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://localhost/aip_dev".into()))
}

pub fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))
}
