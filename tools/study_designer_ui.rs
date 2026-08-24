//! The Study Designer UI's own binary — design.md §3 decision 34,
//! `embarch-study-designer/milestone-11.md` §3.3-§3.6.
//!
//! A local web server, own process, serving an interactive table-based
//! `Study` builder. Read-only outside the authoring table itself: it can
//! submit a `Study` and watch it run, but never builds or flashes anything
//! — that stays `embarch-api`'s job, reached here only by shelling out to
//! its existing `run-study`/`study-status` CLI subcommands (milestone-11.md
//! §2's "Core access, submit-and-poll only" call), never a new HTTP client
//! of this binary's own.
//!
//! ```text
//! cargo run --features study-ui,gatt-extract --bin study-designer-ui -- \
//!     --repo <firmware-repo-path> \
//!     [--port 4887] \
//!     [--embarch-api-bin embarch-api] \
//!     [--embarch-api-config <path-to-embarch.toml>] \
//!     [--static-extractor reference-dut]
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::Mutex;

use embarch_study_designer::{
    build_study, merge_actions, ActionRegistry, ZephyrBleDefExtractor, GattConfigExtractor,
    GattServiceInfo, RegisteredAction, RegistryError, RoleChoice, RowAction, StudyResult,
    TableRow,
};

struct Config {
    repo: PathBuf,
    port: u16,
    embarch_api_bin: String,
    embarch_api_config: Option<PathBuf>,
    static_extractor: Option<String>,
}

fn parse_args() -> Result<Config, String> {
    let mut repo: Option<PathBuf> = None;
    let mut port: u16 = 4887;
    let mut embarch_api_bin = "embarch-api".to_string();
    let mut embarch_api_config: Option<PathBuf> = None;
    let mut static_extractor: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => repo = args.next().map(PathBuf::from),
            "--port" => {
                port = args
                    .next()
                    .ok_or_else(|| "--port needs a value".to_string())?
                    .parse()
                    .map_err(|e| format!("--port: {e}"))?
            }
            "--embarch-api-bin" => {
                embarch_api_bin = args.next().ok_or_else(|| "--embarch-api-bin needs a value".to_string())?
            }
            "--embarch-api-config" => embarch_api_config = args.next().map(PathBuf::from),
            "--static-extractor" => static_extractor = args.next(),
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }

    let repo = repo.ok_or_else(|| {
        "usage: study-designer-ui --repo <firmware-repo-path> [--port 4887] \
         [--embarch-api-bin embarch-api] [--embarch-api-config <path>] \
         [--static-extractor reference-dut]"
            .to_string()
    })?;

    Ok(Config { repo, port, embarch_api_bin, embarch_api_config, static_extractor })
}

/// Resolves `--static-extractor <name>` to a real `GattConfigExtractor` —
/// deliberately opt-in and named explicitly, not auto-detected: silently
/// assuming a firmware matches `ZephyrBleDefExtractor`'s own narrow,
/// one-project conventions (design.md §3 decision 33) for an unrelated repo
/// would be exactly the kind of guess this whole feature exists to avoid.
/// Today there's exactly one real implementation; unrecognized names are a
/// startup error naming what's actually supported, not a silent no-op.
fn resolve_static_extractor(name: &str) -> Result<Box<dyn GattConfigExtractor + Send + Sync>, String> {
    match name {
        "zephyr-ble-def" => Ok(Box::new(ZephyrBleDefExtractor)),
        other => Err(format!(
            "unknown --static-extractor '{other}' — the only one this build ships is 'reference-dut' \
             (embarch-study-designer/design.md §3 decision 33); omit --static-extractor to skip \
             static extraction entirely and rely on live discovery plus the registry alone"
        )),
    }
}

struct AppState {
    repo: PathBuf,
    embarch_api_bin: String,
    embarch_api_config: Option<PathBuf>,
    static_services: Option<Vec<GattServiceInfo>>,
    live_services: Mutex<Option<Vec<GattServiceInfo>>>,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = parse_args().map_err(|e| {
        eprintln!("{e}");
        e
    })?;

    let static_services = match &config.static_extractor {
        Some(name) => {
            let extractor = resolve_static_extractor(name)?;
            let services = extractor.extract(&config.repo).map_err(|e| {
                format!("static extraction failed: {e} (--repo {})", config.repo.display())
            })?;
            Some(services.into_iter().collect())
        }
        None => None,
    };

    let state = Arc::new(AppState {
        repo: config.repo,
        embarch_api_bin: config.embarch_api_bin,
        embarch_api_config: config.embarch_api_config,
        static_services,
        live_services: Mutex::new(None),
    });
    let port = config.port;

    // `embarch-api/design.md` §3 decision 36's own real finding, applied
    // here up front rather than rediscovered the hard way a second time:
    // deserializing a `StudyResult` (this binary's own `/api/study/{id}
    // /status` handler does exactly that, via `embarch_api_json_status`
    // below) overflows a plain tokio worker thread's default stack on a
    // debug build — `Builder::thread_stack_size` alone doesn't cover it,
    // since that only sizes threads the runtime itself spawns, not the
    // thread that calls `block_on`. Same fix: spawn the whole runtime on a
    // dedicated big-stack thread.
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(512 * 1024 * 1024)
                .build()
                .expect("failed to build the tokio runtime")
                .block_on(serve(state, port))
        })?
        .join()
        .map_err(|_| "study-designer-ui's main worker thread panicked")?
}

async fn serve(state: Arc<AppState>, port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/discover", post(discover))
        .route("/api/actions", post(register_action))
        .route("/api/study/run", post(run_study_handler))
        .route("/api/study/{id}/status", get(study_status_handler))
        .with_state(state);

    // Loopback-only, same reasoning as embarch-topology's own local UI
    // (embarch-topology/design.md §3 decision 5's amendment): no TLS, no
    // reason to expose past localhost.
    let addr = format!("127.0.0.1:{port}");
    println!("embarch-study-designer UI listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn load_registry(state: &AppState) -> Result<ActionRegistry, RegistryError> {
    ActionRegistry::load(&state.repo)
}

fn merged_actions_json(state: &AppState, live: Option<&[GattServiceInfo]>) -> serde_json::Value {
    let registry = load_registry(state).unwrap_or_default();
    let actions = merge_actions(live, state.static_services.as_deref(), &registry);
    serde_json::json!({ "actions": actions, "registry": registry })
}

async fn index(State(state): State<Arc<AppState>>) -> Html<String> {
    let live = state.live_services.lock().await;
    let payload = merged_actions_json(&state, live.as_deref());
    Html(page(&payload))
}

async fn discover(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let study = discovery_study();
    let study_json = match serde_json::to_string(&study) {
        Ok(j) => j,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
        }
    };

    match run_and_await(&state, &study_json).await {
        Ok(result) => {
            let services = first_gatt_services(&result);
            if let Some(services) = &services {
                *state.live_services.lock().await = Some(services.clone());
            }
            let live = state.live_services.lock().await;
            (StatusCode::OK, Json(merged_actions_json(&state, live.as_deref())))
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({ "error": e }))),
    }
}

/// The one-step-per-half discovery `Study` `/api/discover` submits — a
/// plain `BleConnect` then `GattDiscover`, the same shape this milestone's
/// own real-hardware validation already used by hand.
fn discovery_study() -> embarch_study_designer::Study {
    let rows = vec![
        TableRow {
            name: "connect".to_string(),
            action: RowAction::BuiltIn {
                which: embarch_study_designer::BuiltInActionKind::BleConnect,
                role: RoleChoice::Central,
            },
            timeout_ms: 20_000,
            continue_on_fail: false,
        },
        TableRow {
            name: "discover".to_string(),
            action: RowAction::BuiltIn {
                which: embarch_study_designer::BuiltInActionKind::GattDiscover,
                role: RoleChoice::Central,
            },
            timeout_ms: 20_000,
            continue_on_fail: false,
        },
    ];
    build_study("study-designer-ui-discover", &rows, &ActionRegistry::default())
        .expect("a two-built-in-step discovery Study always fits within limits")
}

/// The first non-empty `gatt_services` across a `StudyResult`'s steps —
/// `GattDiscover`/`GattMonitorAll` are the only actions that ever populate
/// it, and a discovery run only ever has one such step anyway.
fn first_gatt_services(result: &StudyResult) -> Option<Vec<GattServiceInfo>> {
    result.steps.iter().find_map(|s| s.gatt_services.as_ref().map(|v| v.iter().cloned().collect()))
}

#[derive(serde::Deserialize)]
struct RegisterActionRequest {
    #[serde(flatten)]
    action: RegisteredAction,
}

async fn register_action(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterActionRequest>,
) -> impl IntoResponse {
    let mut registry = match load_registry(&state) {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))),
    };

    if let Some(existing) = registry.actions.iter_mut().find(|a| a.name == req.action.name) {
        *existing = req.action;
    } else {
        registry.actions.push(req.action);
    }

    if let Err(e) = registry.save(&state.repo) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})));
    }

    let live = state.live_services.lock().await;
    (StatusCode::OK, Json(merged_actions_json(&state, live.as_deref())))
}

#[derive(serde::Deserialize)]
struct RunStudyRequest {
    name: String,
    rows: Vec<TableRow>,
}

async fn run_study_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RunStudyRequest>,
) -> impl IntoResponse {
    let registry = match load_registry(&state) {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))),
    };

    let study = match build_study(&req.name, &req.rows, &registry) {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()}))),
    };

    let study_json = match serde_json::to_string(&study) {
        Ok(j) => j,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
        }
    };

    match submit_study(&state, &study_json).await {
        Ok(study_id) => (StatusCode::OK, Json(serde_json::json!({ "study_id": study_id }))),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({ "error": e }))),
    }
}

async fn study_status_handler(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    match embarch_api_json(&state, &["study-status", &id]).await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({ "error": e }))),
    }
}

/// Writes `study_json` to a temp file and runs `embarch-api run-study
/// --study-file <path>`, returning the new `study_id`.
async fn submit_study(state: &AppState, study_json: &str) -> Result<String, String> {
    let path = std::env::temp_dir().join(format!(
        "study-designer-ui-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    tokio::fs::write(&path, study_json).await.map_err(|e| format!("failed to write study file: {e}"))?;

    let result = embarch_api_json(state, &["run-study", "--study-file", path.to_string_lossy().as_ref()]).await;
    let _ = tokio::fs::remove_file(&path).await;

    let value = result?;
    if value.get("success").and_then(|v| v.as_bool()) != Some(true) {
        let msg = value.get("error").and_then(|v| v.as_str()).unwrap_or("run-study failed");
        return Err(msg.to_string());
    }
    value
        .get("study_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "run-study succeeded but returned no study_id".to_string())
}

/// Submits `study_json` (via [`submit_study`]) then polls `study-status`
/// until it reaches a terminal state, returning the final `StudyResult`.
/// Used only by `/api/discover` — the ordinary Study Designer table's own
/// "Run" button (`/api/study/run`) returns immediately and lets the
/// browser do its own polling instead, so a long-running study doesn't
/// hold this handler (and the one worker thread serving it) open the
/// entire time.
async fn run_and_await(state: &AppState, study_json: &str) -> Result<StudyResult, String> {
    let study_id = submit_study(state, study_json).await?;

    for _ in 0..60 {
        let status = embarch_api_json(state, &["study-status", &study_id]).await?;
        let terminal = matches!(status.get("status").and_then(|v| v.as_str()), Some("completed") | Some("failed"));
        if terminal {
            return serde_json::from_value::<Option<StudyResult>>(status.get("result").cloned().unwrap_or_default())
                .map_err(|e| format!("failed to parse study result: {e}"))?
                .ok_or_else(|| {
                    status
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("study finished with no result")
                        .to_string()
                });
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err(format!("study {study_id} did not reach a terminal state within 30s"))
}

/// Runs `embarch-api --json <args...>` (plus `--config`, if configured) as
/// a subprocess and parses its stdout as JSON — the one place this binary
/// ever reaches `embarch-core`, always through `embarch-api`'s own existing
/// CLI, never a new HTTP client of this binary's own (milestone-11.md §2).
async fn embarch_api_json(state: &AppState, args: &[&str]) -> Result<serde_json::Value, String> {
    let mut cmd = tokio::process::Command::new(&state.embarch_api_bin);
    if let Some(config) = &state.embarch_api_config {
        cmd.arg("--config").arg(config);
    }
    cmd.arg("--json");
    cmd.args(args);

    let output = cmd.output().await.map_err(|e| {
        format!("failed to run '{}': {e} (is embarch-api on PATH, or set --embarch-api-bin?)", state.embarch_api_bin)
    })?;

    serde_json::from_slice(&output.stdout).map_err(|e| {
        format!(
            "failed to parse embarch-api's output as JSON: {e} (stdout: {:?}, stderr: {:?})",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// The whole page: a `<script>`-embedded snapshot of the merged action list
/// plus the registry (for building per-field pick-lists), and a vanilla-JS
/// table editor over it — no build step, no framework, matching this
/// suite's own "plain, unstyled HTML" precedent (`embarch-topology/bin/ui.rs`).
fn page(initial: &serde_json::Value) -> String {
    let initial_json = serde_json::to_string(initial).unwrap_or_else(|_| "{}".to_string());
    format!(
        r##"<!doctype html><html><head><title>embarch-study-designer</title>
<meta charset="utf-8">
<style>
body {{ font-family: monospace; max-width: 70rem; margin: 2rem auto; }}
table {{ border-collapse: collapse; width: 100%; margin-bottom: 1rem; }}
td, th {{ border: 1px solid #ccc; padding: 0.4rem; text-align: left; }}
button {{ font-family: monospace; }}
fieldset {{ margin-bottom: 1rem; }}
.error {{ color: #b00; }}
.ok {{ color: #070; }}
</style></head>
<body>
<h1>Study Designer</h1>

<section>
<h2>Steps</h2>
<table id="steps"><thead><tr>
<th>Name</th><th>Action</th><th>Params</th><th>Timeout (ms)</th><th>Continue on fail</th><th></th>
</tr></thead><tbody></tbody></table>
<button onclick="addRow()">+ Add step</button>
<button onclick="discover()">Discover live GATT</button>
<button onclick="runStudy()">Run</button>
<span id="run-status"></span>
</section>

<section>
<h2>Register a custom action</h2>
<p>Pick a detected-but-unregistered characteristic, name it, and give each field its clickable choices — never raw bytes.</p>
<div id="register-form"></div>
</section>

<pre id="log"></pre>

<script>
let STATE = {initial_json};

function el(tag, attrs, children) {{
  const e = document.createElement(tag);
  for (const k in (attrs || {{}})) e.setAttribute(k, attrs[k]);
  for (const c of (children || [])) e.appendChild(typeof c === 'string' ? document.createTextNode(c) : c);
  return e;
}}

function actionLabel(a) {{
  if (a.BuiltIn) return 'BuiltIn: ' + a.BuiltIn;
  if (a.Registered) return a.Registered.name;
  if (a.Unregistered) return '(unregistered) ' + a.Unregistered.uuid.join(',');
  return JSON.stringify(a);
}}

function actionKey(a) {{
  if (a.BuiltIn) return 'builtin:' + a.BuiltIn;
  if (a.Registered) return 'registered:' + a.Registered.name;
  if (a.Unregistered) return 'unregistered:' + a.Unregistered.uuid.join(',');
  return 'unknown';
}}

let rows = [];

function addRow() {{
  rows.push({{ name: 'step' + (rows.length + 1), actionKey: null, role: 'central', field_choices: {{}}, timeout_ms: 15000, continue_on_fail: false }});
  render();
}}

function removeRow(i) {{ rows.splice(i, 1); render(); }}

function pickableActions() {{
  return STATE.actions.filter(a => !a.Unregistered);
}}

function findAction(key) {{
  return STATE.actions.find(a => actionKey(a) === key);
}}

function render() {{
  const tbody = document.querySelector('#steps tbody');
  tbody.innerHTML = '';
  rows.forEach((row, i) => {{
    const tr = document.createElement('tr');

    const nameInput = el('input', {{ value: row.name, size: 12 }});
    nameInput.oninput = () => {{ row.name = nameInput.value; }};
    tr.appendChild(el('td', {{}}, [nameInput]));

    const select = el('select');
    select.appendChild(el('option', {{ value: '' }}, ['-- pick --']));
    pickableActions().forEach(a => {{
      const key = actionKey(a);
      const opt = el('option', {{ value: key }}, [actionLabel(a)]);
      if (key === row.actionKey) opt.setAttribute('selected', 'selected');
      select.appendChild(opt);
    }});
    select.onchange = () => {{ row.actionKey = select.value; row.field_choices = {{}}; render(); }};
    tr.appendChild(el('td', {{}}, [select]));

    const paramsTd = el('td');
    const action = row.actionKey ? findAction(row.actionKey) : null;
    if (action && action.BuiltIn === 'ble_connect') {{
      const roleSelect = el('select');
      ['central', 'peripheral'].forEach(r => {{
        const opt = el('option', {{ value: r }}, [r]);
        if (r === row.role) opt.setAttribute('selected', 'selected');
        roleSelect.appendChild(opt);
      }});
      roleSelect.onchange = () => {{ row.role = roleSelect.value; }};
      paramsTd.appendChild(roleSelect);
    }} else if (action && action.Registered) {{
      (action.Registered.fields || []).forEach(field => {{
        const label = el('label', {{}}, [field.name + ': ']);
        const fieldSelect = el('select');
        fieldSelect.appendChild(el('option', {{ value: '' }}, ['-- pick --']));
        field.values.forEach(v => {{
          const opt = el('option', {{ value: v.label }}, [v.label]);
          if (row.field_choices[field.name] === v.label) opt.setAttribute('selected', 'selected');
          fieldSelect.appendChild(opt);
        }});
        fieldSelect.onchange = () => {{ row.field_choices[field.name] = fieldSelect.value; }};
        paramsTd.appendChild(label);
        paramsTd.appendChild(fieldSelect);
      }});
    }}
    tr.appendChild(paramsTd);

    const timeoutInput = el('input', {{ value: row.timeout_ms, size: 6 }});
    timeoutInput.oninput = () => {{ row.timeout_ms = parseInt(timeoutInput.value, 10) || 0; }};
    tr.appendChild(el('td', {{}}, [timeoutInput]));

    const contCheckbox = el('input', {{ type: 'checkbox' }});
    if (row.continue_on_fail) contCheckbox.setAttribute('checked', 'checked');
    contCheckbox.onchange = () => {{ row.continue_on_fail = contCheckbox.checked; }};
    tr.appendChild(el('td', {{}}, [contCheckbox]));

    const removeBtn = el('button', {{}}, ['remove']);
    removeBtn.onclick = () => removeRow(i);
    tr.appendChild(el('td', {{}}, [removeBtn]));

    tbody.appendChild(tr);
  }});
  renderRegisterForm();
}}

function log(msg, ok) {{
  const pre = document.getElementById('log');
  const line = document.createElement('div');
  line.className = ok ? 'ok' : 'error';
  line.textContent = msg;
  pre.prepend(line);
}}

async function discover() {{
  log('discovering live GATT table...', true);
  const resp = await fetch('/api/discover', {{ method: 'POST' }});
  const body = await resp.json();
  if (!resp.ok) {{ log('discover failed: ' + (body.error || resp.statusText), false); return; }}
  STATE = body;
  log('discover complete', true);
  render();
}}

function rowToTableRow(row) {{
  const action = findAction(row.actionKey);
  if (!action) return null;
  if (action.BuiltIn) {{
    return {{ name: row.name, timeout_ms: row.timeout_ms, continue_on_fail: row.continue_on_fail,
      action: {{ kind: 'built_in', which: action.BuiltIn, role: row.role }} }};
  }}
  return {{ name: row.name, timeout_ms: row.timeout_ms, continue_on_fail: row.continue_on_fail,
    action: {{ kind: 'registered', name: action.Registered.name, field_choices: row.field_choices }} }};
}}

async function runStudy() {{
  const tableRows = rows.map(rowToTableRow);
  if (tableRows.some(r => r === null)) {{ log('every step needs an action picked', false); return; }}
  const resp = await fetch('/api/study/run', {{
    method: 'POST', headers: {{ 'Content-Type': 'application/json' }},
    body: JSON.stringify({{ name: 'study-designer-ui', rows: tableRows }})
  }});
  const body = await resp.json();
  if (!resp.ok) {{ log('run failed: ' + (body.error || resp.statusText), false); return; }}
  log('submitted: ' + body.study_id, true);
  pollStatus(body.study_id);
}}

async function pollStatus(id) {{
  const span = document.getElementById('run-status');
  const resp = await fetch('/api/study/' + id + '/status');
  const body = await resp.json();
  // Two distinct failure shapes to handle, not one: a transport/HTTP-level
  // failure (!resp.ok, this binary's own handler returning an error), and
  // embarch-api's own `{{success: false, error: ...}}` passthrough shape
  // (e.g. an unknown study_id) -- the latter still comes back as plain 200
  // OK, since this handler forwards embarch-api's JSON unchanged rather
  // than reinterpreting it. Either one needs to stop polling, not loop
  // forever waiting for a `status` field that will never arrive.
  if (!resp.ok || body.success === false) {{ log('status failed: ' + (body.error || resp.statusText), false); return; }}
  span.textContent = 'status: ' + body.status + ' (' + (body.current_step ?? '?') + '/' + (body.total_steps ?? '?') + ')';
  if (body.status === 'completed' || body.status === 'failed') {{
    log('study ' + id + ' ' + body.status + ': ' + JSON.stringify(body.result || body.reason), body.status === 'completed');
    return;
  }}
  setTimeout(() => pollStatus(id), 1000);
}}

let regFields = [];

function renderRegisterForm() {{
  const container = document.getElementById('register-form');
  container.innerHTML = '';
  const unregistered = STATE.actions.filter(a => a.Unregistered);
  if (unregistered.length === 0) {{
    container.appendChild(el('p', {{}}, ['nothing detected yet — try "Discover live GATT" above']));
    return;
  }}

  const charSelect = el('select', {{ id: 'reg-char' }});
  unregistered.forEach((a, i) => {{
    const u = a.Unregistered;
    charSelect.appendChild(el('option', {{ value: i }}, [u.uuid.join(',') + ' (service ' + u.service_uuid.join(',') + ')']));
  }});
  container.appendChild(el('label', {{}}, ['Characteristic: ', charSelect]));
  container.appendChild(document.createElement('br'));

  const nameInput = el('input', {{ id: 'reg-name', placeholder: 'action name' }});
  container.appendChild(el('label', {{}}, ['Name: ', nameInput]));
  container.appendChild(document.createElement('br'));

  const opSelect = el('select', {{ id: 'reg-op' }});
  ['read', 'write', 'subscribe', 'notify', 'indicate'].forEach(op => opSelect.appendChild(el('option', {{ value: op }}, [op])));
  container.appendChild(el('label', {{}}, ['Operation: ', opSelect]));
  container.appendChild(document.createElement('br'));

  const fieldsDiv = el('div', {{ id: 'reg-fields' }});
  container.appendChild(fieldsDiv);

  const addFieldBtn = el('button', {{ type: 'button' }}, ['+ Add field']);
  addFieldBtn.onclick = () => {{ regFields.push({{ name: '', byte_offset: 0, byte_len: 1, values: [] }}); renderRegFields(); }};
  container.appendChild(addFieldBtn);
  container.appendChild(document.createElement('br'));

  const submitBtn = el('button', {{ type: 'button' }}, ['Register action']);
  submitBtn.onclick = () => submitRegistration(unregistered, parseInt(charSelect.value, 10));
  container.appendChild(submitBtn);

  renderRegFields();
}}

function renderRegFields() {{
  const div = document.getElementById('reg-fields');
  if (!div) return;
  div.innerHTML = '';
  regFields.forEach((field, fi) => {{
    const wrap = el('fieldset');
    const nameInput = el('input', {{ value: field.name, placeholder: 'field name' }});
    nameInput.oninput = () => {{ field.name = nameInput.value; }};
    const offsetInput = el('input', {{ value: field.byte_offset, size: 3 }});
    offsetInput.oninput = () => {{ field.byte_offset = parseInt(offsetInput.value, 10) || 0; }};
    const lenInput = el('input', {{ value: field.byte_len, size: 3 }});
    lenInput.oninput = () => {{ field.byte_len = parseInt(lenInput.value, 10) || 1; }};
    wrap.appendChild(el('label', {{}}, ['name: ', nameInput]));
    wrap.appendChild(el('label', {{}}, [' offset: ', offsetInput]));
    wrap.appendChild(el('label', {{}}, [' length: ', lenInput]));

    const valuesDiv = el('div');
    field.values.forEach((val, vi) => {{
      const labelInput = el('input', {{ value: val.label, placeholder: 'label' }});
      labelInput.oninput = () => {{ val.label = labelInput.value; }};
      const bytesInput = el('input', {{ value: val.bytesText || '', placeholder: 'bytes, e.g. 0x01 or 1,0' }});
      bytesInput.oninput = () => {{ val.bytesText = bytesInput.value; }};
      valuesDiv.appendChild(el('div', {{}}, [labelInput, bytesInput]));
    }});
    const addValueBtn = el('button', {{ type: 'button' }}, ['+ value']);
    addValueBtn.onclick = () => {{ field.values.push({{ label: '', bytesText: '' }}); renderRegFields(); }};
    wrap.appendChild(valuesDiv);
    wrap.appendChild(addValueBtn);
    div.appendChild(wrap);
  }});
}}

function parseBytes(text) {{
  return text.split(/[,\s]+/).filter(t => t.length > 0).map(tok => {{
    return tok.toLowerCase().startsWith('0x') ? parseInt(tok, 16) : parseInt(tok, 10);
  }});
}}

async function submitRegistration(unregistered, idx) {{
  const u = unregistered[idx].Unregistered;
  const name = document.getElementById('reg-name').value;
  const operation = document.getElementById('reg-op').value;
  const fields = regFields.map(f => ({{
    name: f.name, byte_offset: f.byte_offset, byte_len: f.byte_len,
    values: f.values.map(v => ({{ label: v.label, bytes: parseBytes(v.bytesText || '') }}))
  }}));
  const body = {{ name, service_uuid: u.service_uuid, uuid: u.uuid, operation, fields: operation === 'write' ? fields : [] }};
  const resp = await fetch('/api/actions', {{
    method: 'POST', headers: {{ 'Content-Type': 'application/json' }}, body: JSON.stringify(body)
  }});
  const respBody = await resp.json();
  if (!resp.ok) {{ log('register failed: ' + (respBody.error || resp.statusText), false); return; }}
  STATE = respBody;
  regFields = [];
  log('registered action: ' + name, true);
  render();
}}

addRow();
render();
</script>
</body></html>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_study_is_ble_connect_then_gatt_discover() {
        let study = discovery_study();
        assert_eq!(study.steps.len(), 2);
        assert!(matches!(study.steps[0].action, embarch_study_designer::Action::BleConnect { .. }));
        assert!(matches!(study.steps[1].action, embarch_study_designer::Action::GattDiscover {}));
    }

    #[test]
    fn resolve_static_extractor_recognizes_eight_sleep() {
        assert!(resolve_static_extractor("zephyr-ble-def").is_ok());
    }

    #[test]
    fn resolve_static_extractor_names_unknown_extractors_rather_than_silently_picking_one() {
        let err = match resolve_static_extractor("some-other-firmware") {
            Err(e) => e,
            Ok(_) => panic!("expected an error for an unknown extractor name"),
        };
        assert!(err.contains("some-other-firmware"));
        assert!(err.contains("zephyr-ble-def"));
    }

    #[test]
    fn first_gatt_services_finds_the_populated_step() {
        // Same fix as embarch-study-designer's own crate-level tests
        // (`src/lib.rs`, `src/study_builder.rs`): `StudyResult` embeds a
        // `heapless::Vec<StepResult, MAX_STEPS_PER_STUDY>` -- a fixed-size
        // *inline* array sized for all 64 slots regardless of how many this
        // test actually populates -- so even just constructing one on a
        // debug build's default test-thread stack overflows (confirmed by
        // hitting it for real, not assumed).
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(first_gatt_services_finds_the_populated_step_body)
            .expect("failed to spawn test thread")
            .join()
            .expect("first_gatt_services_finds_the_populated_step body panicked");
    }

    fn first_gatt_services_finds_the_populated_step_body() {
        use embarch_study_designer::{GattCharacteristicInfo, GattServiceInfo, Outcome, StepResult};
        let services: heapless::Vec<GattServiceInfo, { embarch_study_designer_limits_max() }> = {
            let mut v = heapless::Vec::new();
            let _ = v.push(GattServiceInfo {
                uuid: embarch_study_designer::Uuid([1; 16]),
                characteristics: {
                    let mut c = heapless::Vec::new();
                    let _ = c.push(GattCharacteristicInfo { uuid: embarch_study_designer::Uuid([2; 16]), properties: 2 });
                    c
                },
            });
            v
        };
        let result = StudyResult {
            study_name: heapless::String::try_from("t").unwrap(),
            steps: {
                let mut steps = heapless::Vec::new();
                let _ = steps.push(StepResult {
                    step_name: heapless::String::try_from("connect").unwrap(),
                    outcome: Outcome::Pass,
                    captured_data: None,
                    power_samples_ref: None,
                    waveform_ref: None,
                    gatt_services: None,
                    gatt_activity: None,
                });
                let _ = steps.push(StepResult {
                    step_name: heapless::String::try_from("discover").unwrap(),
                    outcome: Outcome::Pass,
                    captured_data: None,
                    power_samples_ref: None,
                    waveform_ref: None,
                    gatt_services: Some(services),
                    gatt_activity: None,
                });
                steps
            },
            validations: heapless::Vec::new(),
        };
        let found = first_gatt_services(&result).unwrap();
        assert_eq!(found.len(), 1);
    }

    const fn embarch_study_designer_limits_max() -> usize {
        embarch_study_designer::limits::MAX_DISCOVERED_SERVICES
    }
}
