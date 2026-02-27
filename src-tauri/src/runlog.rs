use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::Manager;

use crate::interceptor::ContextSnapshot;

static RUN_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
pub struct RunHandle {
    pub id: String,
    pub dir: PathBuf,
    pub started_ms: u128,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn runs_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    Ok(base.join("runs"))
}

fn write_text(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, contents).map_err(|e| e.to_string())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    write_text(path, &text)
}

fn append_trace(dir: &Path, line: &str) -> Result<(), String> {
    let path = dir.join("trace.jsonl");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    use std::io::Write;
    file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize)]
struct RunMeta<'a> {
    run_id: &'a str,
    started_ms: u128,
    pid: u32,
    stage: &'a str,
}

#[derive(Serialize)]
struct TraceEvent<'a> {
    event: &'a str,
    run_id: &'a str,
    ts_ms: u128,
}

pub fn start_run(
    app: &tauri::AppHandle,
    input: &str,
    ctx: Option<&ContextSnapshot>,
) -> Result<RunHandle, String> {
    let started_ms = now_ms();
    let pid = std::process::id();
    let counter = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let run_id = format!("run-{started_ms}-{pid}-{counter}");
    let run_dir = runs_root(app)?.join(&run_id);
    fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;

    let meta = RunMeta {
        run_id: &run_id,
        started_ms,
        pid,
        stage: "phase1",
    };
    write_json(&run_dir.join("meta.json"), &meta)?;
    write_json(&run_dir.join("context.json"), &ctx)?;
    write_text(&run_dir.join("input.txt"), input)?;
    let trace = TraceEvent {
        event: "run_start",
        run_id: &run_id,
        ts_ms: started_ms,
    };
    let trace_line = serde_json::to_string(&trace).map_err(|e| e.to_string())?;
    append_trace(&run_dir, &trace_line)?;

    Ok(RunHandle {
        id: run_id,
        dir: run_dir,
        started_ms,
    })
}

pub fn write_plan<T: Serialize>(run: &RunHandle, plan: &T) -> Result<(), String> {
    write_json(&run.dir.join("plan.json"), plan)
}

pub fn write_hard_policy<T: Serialize>(run: &RunHandle, policy: &T) -> Result<(), String> {
    write_json(&run.dir.join("hard_policy.json"), policy)
}

pub fn write_artifact(run: &RunHandle, name: &str, contents: &str) -> Result<(), String> {
    let path = run.dir.join("artifacts").join(name);
    write_text(&path, contents)
}

pub fn write_result(
    run: &RunHandle,
    text: &str,
    debug: Option<&serde_json::Value>,
) -> Result<(), String> {
    write_text(&run.dir.join("final.txt"), text)?;
    if let Some(value) = debug {
        write_json(&run.dir.join("debug.json"), value)?;
    }
    let trace = TraceEvent {
        event: "run_done",
        run_id: &run.id,
        ts_ms: now_ms(),
    };
    let trace_line = serde_json::to_string(&trace).map_err(|e| e.to_string())?;
    append_trace(&run.dir, &trace_line)?;
    Ok(())
}

pub fn write_error(run: &RunHandle, error: &str) -> Result<(), String> {
    write_text(&run.dir.join("error.txt"), error)?;
    let trace = TraceEvent {
        event: "run_error",
        run_id: &run.id,
        ts_ms: now_ms(),
    };
    let trace_line = serde_json::to_string(&trace).map_err(|e| e.to_string())?;
    append_trace(&run.dir, &trace_line)?;
    Ok(())
}
