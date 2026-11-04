//! Local-only, seven-process WCS/RCS/ECS prototype. No ROS, PLC, or production endpoints.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
mod catalog;
mod navigation;
mod traffic;
mod yielding;
mod workflow;
use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const INTEGRATION: u16 = 8210;
const RCS: u16 = 8211;
const VEHICLES: [(&str, u16); 3] = [
    ("CT-SIM-01", 8212),
    ("CT-SIM-02", 8213),
    ("CT-SIM-03", 8214),
];
const PARKING_NODES: [&str; 5] = ["P1", "P2", "P3", "I1", "I2"];
static VEHICLE_ID: OnceLock<String> = OnceLock::new();
fn vehicle_id() -> &'static str {
    VEHICLE_ID.get().map(String::as_str).unwrap_or("CT-SIM-01")
}
fn default_kind() -> String {
    "TRANSPORT".into()
}
fn default_vehicle_id() -> String {
    "CT-SIM-01".into()
}
fn vehicle_port(id: &str) -> Option<u16> {
    VEHICLES.iter().find(|v| v.0 == id).map(|v| v.1)
}
fn fleet_states() -> Vec<Value> {
    VEHICLES
        .iter()
        .map(
            |(id, port)| match http(*port, "GET", "/api/state", Value::Null) {
                Ok(mut v) => {
                    v["port"] = json!(port);
                    v
                }
                Err(e) => json!({"vehicle_id":id,"port":port,"online":false,"error":e}),
            },
        )
        .collect()
}
const NOTES_UI:&str=include_str!("../web/notes.html");
const ARCHITECTURE_UI: &str = include_str!("../web/architecture.html");
const UI: &str = include_str!("../web/index.html");
const TRAFFIC_UI: &str = include_str!("../web/traffic.html");
const TRAFFIC_SCRIPT: &str = include_str!("../web/traffic.js");
const CATALOG_UI: &str = include_str!("../web/catalog.html");
const FLEET_UI: &str = include_str!("../web/fleet.html");
const STATION_UI: &str = include_str!("../web/stations.html");
const STATION_SCRIPT: &str = include_str!("../web/stations.js");
const FACTORY_SCRIPT: &str = include_str!("../web/factory.js");
const ROAD_SCRIPT: &str = include_str!("../web/roads.js");
const CONSOLE_SCRIPT: &str = include_str!("../web/console.js");
const WORKFLOW_UI: &str = include_str!("../web/workflow.html");
const WORKFLOW_SCRIPT: &str = include_str!("../web/workflow.js");
const APP_THEME: &str = include_str!("../web/theme.css");
const APP_SHELL: &str = include_str!("../web/shell.html");
fn app_shell(role: &str) -> String {
    APP_SHELL.replace("__AGV_PORT__", &if role == "agv" { vehicle_port(vehicle_id()).unwrap_or(8212) } else { 8212 }.to_string())
        .replace(&format!("data-service=\"{role}\""), &format!("data-service=\"{role}\" aria-current=\"page\""))
}
const APP_SCRIPT: &str = include_str!("../web/app.js");
const NODES: [(&str, f64, f64); 18] = [
    ("A", 15., 25.),
    ("B", 80., 25.),
    ("WIP", 80., 78.),
    ("C", 15., 78.),
    ("HOME", 48., 52.),
    ("P1", 30., 55.),
    ("P2", 48., 64.),
    ("P3", 65., 55.),
    ("CUT", 15., 7.),
    ("CNC", 36., 7.),
    ("OVEN", 58., 7.),
    ("PACK", 80., 7.),
    ("CHG1", 96., 38.),
    ("CHG2", 96., 64.),
    ("S1", 15., 94.),
    ("S2", 36., 94.),
    ("I1", 58., 94.),
    ("I2", 80., 94.),
];
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn terminal(s: &str) -> bool {
    matches!(s, "COMPLETED" | "CANCELED" | "FAILED" | "INTERRUPTED")
}
fn node(id: &str) -> Option<(f64, f64)> {
    NODES.iter().find(|n| n.0 == id).map(|n| (n.1, n.2))
}

#[derive(Clone, Serialize, Deserialize)]
struct Task {
    id: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    parent_task: Option<String>,
    #[serde(default)]
    job_id: Option<String>,
    request_id: String,
    from: String,
    to: String,
    status: String,
    progress: u32,
    phase: String,
    created_at: u64,
    updated_at: u64,
    note: String,
    last_seq: u64,
    #[serde(default)]
    assigned_vehicle: Option<String>,
    #[serde(default)]
    requested_vehicle: Option<String>,
    #[serde(default)]
    wait_reason: Option<String>,
    #[serde(default)]
    wait_since: Option<u64>,
    #[serde(default)]
    timing: TaskTiming,
    #[serde(default)]
    routes: Vec<navigation::RouteGrant>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct TaskTiming {
    dispatched_at: Option<u64>,
    accepted_at: Option<u64>,
    first_progress_at: Option<u64>,
    completed_at: Option<u64>,
    canceled_at: Option<u64>,
    interrupted_at: Option<u64>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Event {
    at: u64,
    task_id: String,
    message: String,
    #[serde(default)]
    vehicle_id: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct RcsState {
    #[serde(default)]
    job_cancellations: Vec<workflow::JobCancellation>,
    #[serde(default)]
    job_recoveries: Vec<workflow::recovery::Receipt>,
    #[serde(default)]
    job_locks: Vec<workflow::VehicleLock>,
    tasks: Vec<Task>,
    events: VecDeque<Event>,
    auto_dispatch: bool,
    #[serde(skip)]
    storage_error: Option<String>,
    #[serde(default)]
    traffic: traffic::Traffic,
    #[serde(default)]
    roads: navigation::RoadState,
}
impl Default for RcsState {
    fn default() -> Self {
        Self {
            job_cancellations: vec![],
            job_recoveries: vec![],
            job_locks: vec![],
            tasks: vec![],
            events: VecDeque::new(),
            auto_dispatch: true,
            storage_error: None,
            traffic: traffic::Traffic::default(),
            roads: navigation::RoadState::default(),
        }
    }
}
fn db_path() -> PathBuf {
    std::env::var_os("RCS_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"))
        .join("rcs.json")
}
fn log_event(s: &mut RcsState, id: &str, msg: &str) {
    let vehicle_id = s
        .tasks
        .iter()
        .find(|t| t.id == id)
        .and_then(|t| t.assigned_vehicle.clone());
    s.events.push_front(Event {
        at: now(),
        task_id: id.into(),
        message: msg.into(),
        vehicle_id,
    });
    s.events.truncate(150);
}
fn save(s: &mut RcsState) {
    let p = db_path();
    let r = (|| -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(p.parent().unwrap())?;
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
        fs::rename(tmp, p)?;
        Ok(())
    })();
    s.storage_error = r.err().map(|e| e.to_string());
}
fn load() -> Result<RcsState, String> {
    let p = db_path();
    let mut s: RcsState = match fs::read(&p) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            format!(
                "Cannot parse {}: {e}. Existing file preserved.",
                p.display()
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => RcsState::default(),
        Err(e) => return Err(e.to_string()),
    };
    let mut ids = vec![];
    for t in &mut s.tasks {
        if !terminal(&t.status) && t.status != "QUEUED" {
            t.status = "INTERRUPTED".into();
            t.note = "RCS 重啟，先前執行結果待人工確認；不自動重派。".into();
            t.updated_at = now();
            t.timing.interrupted_at = Some(t.updated_at);
            ids.push(t.id.clone());
        }
    }
    for id in ids {
        log_event(&mut s, &id, "重啟復原：執行中任務標示待確認");
    }
    save(&mut s);
    if let Some(e) = &s.storage_error {
        return Err(format!("RCS storage unavailable: {e}"));
    }
    Ok(s)
}

#[derive(Clone, Serialize, Deserialize)]
struct Mission {
    task_id: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    parent_task: Option<String>,
    from: String,
    to: String,
    #[serde(default = "default_vehicle_id")]
    vehicle_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Callback {
    task_id: String,
    seq: u64,
    status: String,
    progress: u32,
    phase: String,
    #[serde(default = "default_vehicle_id")]
    vehicle_id: String,
}
#[derive(Clone)]
struct Active {
    mission: Mission,
    ticks: u32,
    seq: u64,
    motion: Option<navigation::Motion>,
}
struct Vehicle {
    active: Option<Active>,
    x: f64,
    y: f64,
    paused: bool,
    last_task: Option<Callback>,
    seen: VecDeque<String>,
    pending: VecDeque<Callback>,
    phase: String,
    progress: u32,
    callback_error: Option<String>,
    waiting_for: Option<String>,
    route_path: Vec<String>,
    route_token: Option<String>,
    road_edge: Option<(String, String)>,
}
impl Default for Vehicle {
    fn default() -> Self {
        Self {
            active: None,
            x: match vehicle_id() {
                "CT-SIM-02" => 48.,
                "CT-SIM-03" => 65.,
                _ => 30.,
            },
            y: if vehicle_id() == "CT-SIM-02" {
                64.
            } else {
                55.
            },
            paused: false,
            last_task: None,
            seen: VecDeque::new(),
            pending: VecDeque::new(),
            phase: "待命".into(),
            progress: 0,
            callback_error: None,
            waiting_for: None,
            route_path: vec![],
            route_token: None,
            road_edge: None,
        }
    }
}
type Shared<T> = Arc<Mutex<T>>;

static ROLE: OnceLock<String> = OnceLock::new();
static TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static TRACES: OnceLock<Mutex<VecDeque<ApiTrace>>> = OnceLock::new();
#[derive(Clone, Serialize)]
struct ApiTrace {
    id: String,
    at: u64,
    received_at: u64,
    response_ready_at: u64,
    responded_at: Option<u64>,
    source: String,
    target: String,
    port: u16,
    method: String,
    path: String,
    task_id: Option<String>,
    job_id: Option<String>,
    command_id: Option<String>,
    vehicle_id: Option<String>,
    request: Value,
    response: Value,
    status: u16,
    duration_ms: f64,
    write_error: Option<String>,
}
fn trace_store() -> &'static Mutex<VecDeque<ApiTrace>> {
    TRACES.get_or_init(|| Mutex::new(VecDeque::new()))
}
fn trace_value(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        return Value::Null;
    }
    if bytes.len() > 65536 {
        return json!({"_notice":"內容超過 64 KiB，僅顯示預覽", "preview":String::from_utf8_lossy(bytes).chars().take(4000).collect::<String>()});
    }
    serde_json::from_slice(bytes).unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(bytes)}))
}
fn record_trace(
    role: &str,
    port: u16,
    req: &Request,
    res: &Response,
    at: u64,
    response_ready_at: u64,
    responded_at: Option<u64>,
    duration_ms: f64,
    write_error: Option<String>,
) {
    if !req.path.starts_with("/api/")
        || matches!(
            req.path.as_str(),
            "/api/snapshot" | "/api/traces" | "/api/catalog" | "/api/dashboard" | "/api/workflow"
        )
    {
        return;
    }
    let mut traces = trace_store().lock().unwrap();
    // Sample repeated state polling; keep every task/control request and callback.
    if req.method == "GET"
        && traces.iter().any(|t| {
            t.method == "GET"
                && t.source == req.source
                && t.path == req.path
                && at.saturating_sub(t.at) < 2000
        })
    {
        return;
    }
    let request = trace_value(&req.body);
    let response = trace_value(&res.body);
    let task_id = request["task_id"]
        .as_str()
        .or(response["task"]["id"].as_str())
        .map(str::to_owned)
        .or_else(|| req.path.strip_prefix("/api/cancel/").map(str::to_owned));
    let trace_vehicle = request["vehicle_id"]
        .as_str()
        .or(response["vehicle_id"].as_str())
        .or(response["task"]["assigned_vehicle"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            if role == "agv" {
                Some(vehicle_id().into())
            } else if req.source.starts_with("CT-SIM-") {
                Some(req.source.clone())
            } else {
                None
            }
        });
    let job_id=request["job_id"].as_str().or(response["job"]["id"].as_str())
        .or(response["job_id"].as_str()).or(response["interaction"]["job_id"].as_str()).map(str::to_owned);
    let command_id=request["command_id"].as_str().or(response["interaction"]["command_id"].as_str()).map(str::to_owned);
    traces.push_front(ApiTrace {
        id: format!(
            "{}-{}",
            std::process::id(),
            TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ),
        at,
        received_at: at,
        response_ready_at,
        responded_at,
        source: req.source.clone(),
        target: role.into(),
        port,
        method: req.method.clone(),
        path: req.path.clone(),
        task_id,
        job_id,
        command_id,
        vehicle_id: trace_vehicle,
        request,
        response,
        status: res.code,
        duration_ms,
        write_error,
    });
    // Independent retention: state polling must not evict the task trace immediately.
    let (mut control, mut query) = (0, 0);
    traces.retain(|t| {
        if t.method == "GET" {
            query += 1;
            query <= 20
        } else {
            control += 1;
            control <= 160
        }
    });
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
    source: String,
}
struct Response {
    code: u16,
    body: Vec<u8>,
    content_type: &'static str,
}
fn response(code: u16, value: Value) -> Response {
    Response {
        code,
        body: serde_json::to_vec(&value).unwrap(),
        content_type: "application/json; charset=utf-8",
    }
}
fn ok(value: Value) -> Response {
    response(200, value)
}
fn error(code: u16, msg: &str) -> Response {
    response(code, json!({"error":msg}))
}
fn parse<T: for<'de> Deserialize<'de>>(r: &Request) -> Result<T, Response> {
    serde_json::from_slice(&r.body).map_err(|e| error(400, &format!("資料格式錯誤：{e}")))
}

// Deliberately small local HTTP transport: bounded payload, timeout, one request per connection.
// Bind only loopback. Do not expose this prototype to a network as a production server.
fn read_request(s: &mut TcpStream) -> Result<Request, String> {
    s.set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    let mut data = vec![];
    let end;
    loop {
        let mut b = [0u8; 2048];
        let n = s.read(&mut b).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("Incomplete headers".into());
        }
        data.extend_from_slice(&b[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            end = i + 4;
            break;
        }
        if data.len() > 16384 {
            return Err("Headers too large".into());
        }
    }
    let head = std::str::from_utf8(&data[..end]).map_err(|e| e.to_string())?;
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_string();
    let path = first
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .to_string();
    let mut length = 0usize;
    let mut lengths = 0;
    for l in head.lines().skip(1) {
        if let Some((k, v)) = l.split_once(':') {
            if k.eq_ignore_ascii_case("origin")
                && ![
                    "http://127.0.0.1:8210",
                    "http://127.0.0.1:8211",
                    "http://127.0.0.1:8212",
                    "http://127.0.0.1:8213",
                    "http://127.0.0.1:8214",
                    "http://127.0.0.1:8215",
                    "http://127.0.0.1:8216",
                    "http://localhost:8210",
                    "http://localhost:8211",
                    "http://localhost:8212",
                    "http://localhost:8213",
                    "http://localhost:8214",
                    "http://localhost:8215",
                    "http://localhost:8216",
                ]
                .contains(&v.trim())
            {
                return Err("Only this local simulator's pages may make requests".into());
            }
        }
    }
    if method == "POST"
        && !head.lines().any(|l| {
            l.split_once(':').is_some_and(|(k, v)| {
                k.eq_ignore_ascii_case("content-type") && v.trim().starts_with("application/json")
            })
        })
    {
        return Err("POST requires application/json".into());
    }
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                lengths += 1;
                length = v.trim().parse().map_err(|_| "Invalid content length")?;
            }
            if k.eq_ignore_ascii_case("transfer-encoding") {
                return Err("Chunked requests are not supported".into());
            }
        }
    }
    if lengths > 1 || length > 32768 {
        return Err("Invalid payload size".into());
    }
    let source = head
        .lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("x-lab-source"))
                .map(|(_, v)| v.trim().chars().take(64).collect())
        })
        .unwrap_or_else(|| "browser".into());
    while data.len() - end < length {
        let mut b = [0u8; 2048];
        let n = s.read(&mut b).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("Incomplete body".into());
        }
        data.extend_from_slice(&b[..n]);
    }
    Ok(Request {
        method,
        path,
        body: data[end..end + length].to_vec(),
        source,
    })
}
fn http(port: u16, method: &str, path: &str, body: Value) -> Result<Value, String> {
    let (status, v) = http_response(port, method, path, body)?;
    if status >= 400 {
        Err(v["error"].as_str().unwrap_or("Service error").to_string())
    } else {
        Ok(v)
    }
}
fn http_response(port: u16, method: &str, path: &str, body: Value) -> Result<(u16, Value), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s =
        TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    let b = if method == "GET" {
        vec![]
    } else {
        serde_json::to_vec(&body).map_err(|e| e.to_string())?
    };
    write!(s,"{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Lab-Source: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",ROLE.get().map(String::as_str).unwrap_or("test"),b.len()).map_err(|e|e.to_string())?;
    s.write_all(&b).map_err(|e| e.to_string())?;
    let mut bytes = vec![];
    std::io::Read::by_ref(&mut s)
        .take(2_000_000)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let i = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Invalid HTTP response")?;
    let status = String::from_utf8_lossy(&bytes[..i])
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .parse::<u16>()
        .unwrap_or(500);
    let v: Value = serde_json::from_slice(&bytes[i + 4..]).map_err(|e| e.to_string())?;
    Ok((status, v))
}
fn proxy(port: u16, r: &Request) -> Response {
    let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    match http_response(port, &r.method, &r.path, body) {
        Ok((code, v)) => {
            let mut response = ok(v);
            response.code = code;
            response
        },
        Err(e) => error(502, &e),
    }
}

fn rcs_state_json(state: &Shared<RcsState>) -> Value {
    let s = state.lock().unwrap();
    json!({"role":"rcs","tasks":s.tasks,"events":s.events,"auto_dispatch":s.auto_dispatch,"storage_error":s.storage_error,"stations":s.traffic.view(),"roads":s.roads.view(),"job_locks":s.job_locks,"job_cancellations":s.job_cancellations,"job_recoveries":s.job_recoveries})
}

fn common(role: &str, r: &Request, state: Option<&Shared<RcsState>>) -> Option<Response> {
    if role != "agv" {
        if r.path.starts_with("/api/jobs/") && (r.path.ends_with("/recover")||r.path.ends_with("/continue")) && role=="central" {return Some(error(403,"人工復歸／接續請至內部 WCS 操作頁"));}
        if r.path == "/api/jobs" || r.path.starts_with("/api/jobs/") {
            return Some(proxy(workflow::WCS, r));
        }
        if r.method == "GET" && r.path == "/api/workflow" {
            return Some(match http(workflow::WCS,"GET","/api/dashboard",Value::Null) {
                Ok(v)=>ok(v), Err(e)=>error(502,&e),
            });
        }
    }
    if r.method == "GET" && r.path == "/api/map" {
        return Some(ok(navigation::graph()));
    }
    if r.method == "GET" && r.path == "/api/catalog" {
        return Some(ok(catalog::catalog()));
    }
    if r.method == "GET" && r.path == "/api/traces" {
        return Some(ok(
            json!({"traces":trace_store().lock().unwrap().iter().cloned().collect::<Vec<_>>()}),
        ));
    }
    if r.method == "GET" && r.path == "/" {
        return Some(Response {
            code: 200,
            body: UI
                .replace("<!-- APP-THEME -->", APP_THEME)
                .replace("<!-- APP-SHELL -->", &app_shell(role))
                .replace("<!-- PROJECT-NOTES -->", NOTES_UI)
                .replace("<!-- CURRENT-ARCHITECTURE -->", ARCHITECTURE_UI)
                .replace("<!-- FLEET-UI -->", FLEET_UI)
                .replace("<!-- WORKFLOW-UI -->", WORKFLOW_UI)
                .replace("<!-- STATION-UI -->", STATION_UI)
                .replace(
                    "<!-- APP-SCRIPT -->",
                    &format!(
                        "{}\n{}\n{}\n{}\n{}\n{}",
                        APP_SCRIPT, STATION_SCRIPT, CONSOLE_SCRIPT, FACTORY_SCRIPT, ROAD_SCRIPT, WORKFLOW_SCRIPT
                    ),
                )
                .replace("__ROLE__", role)
                .replace("__VEHICLE_ID__", vehicle_id())
                .replace("<!-- API-TRAFFIC -->", TRAFFIC_UI)
                .replace("<!-- API-TRAFFIC-SCRIPT -->", TRAFFIC_SCRIPT)
                .replace("<!-- API-CATALOG -->", CATALOG_UI)
                .into_bytes(),
            content_type: "text/html; charset=utf-8",
        });
    }
    if r.method == "GET" && r.path == "/health" {
        return Some(ok(
            json!({"ok":true,"role":role,"service_name":if role=="central"{"integration-service"}else{role},"vehicle_id":if role=="agv"{Some(vehicle_id())}else{None},"mode":"simulation","time":now()}),
        ));
    }
    if r.method == "GET" && r.path == "/api/snapshot" {
        // Only RCS aggregates fleet state and diagnostic traces. Other service
        // pages consume that snapshot instead of contacting vehicles directly.
        if role != "rcs" {
            return Some(match http(RCS, "GET", "/api/snapshot", Value::Null) {
                Ok(mut snapshot) => {
                    snapshot["role"] = json!(role);
                    // Central owns ingress diagnostics; RCS never queries central.
                    if let Some(items) = snapshot["traffic"].as_array_mut() {
                        items.extend(trace_store().lock().unwrap().iter().map(|t| json!(t)));
                        items.sort_by(|a,b| b["at"].as_u64().cmp(&a["at"].as_u64()));
                    }
                    ok(snapshot)
                }
                Err(e) => error(502, &e),
            });
        }
        let rcs = rcs_state_json(state.expect("RCS snapshot requires local state"));
        // UI refreshes read the observations maintained by the dispatch loop.
        let vehicles = state.unwrap().lock().unwrap().traffic.observations.clone();
        let current_id = if role == "agv" {
            vehicle_id()
        } else {
            "CT-SIM-01"
        };
        let vehicle = vehicles
            .iter()
            .find(|v| v["vehicle_id"] == current_id && v["online"] == true)
            .cloned();
        let vehicle_error = vehicles
            .iter()
            .find(|v| v["vehicle_id"] == current_id && v["online"] != true)
            .and_then(|v| v["error"].as_str())
            .map(str::to_owned);
        let mut traffic = vec![];
        for port in [RCS, 8212, 8213, 8214] {
            if port == RCS {
                traffic.extend(trace_store().lock().unwrap().iter().map(|trace| json!(trace)));
                continue;
            }
            if let Ok(t) = http(port, "GET", "/api/traces", Value::Null) {
                if let Some(items) = t["traces"].as_array() {
                    traffic.extend(items.iter().cloned())
                }
            }
        }
        traffic.sort_by(|a, b| b["at"].as_u64().cmp(&a["at"].as_u64()));
        return Some(ok(
            json!({"role":role,"rcs":rcs,"rcs_error":null,"vehicle":vehicle,"vehicle_error":vehicle_error,"vehicles":vehicles,"traffic":traffic}),
        ));
    }
    None
}
#[derive(Deserialize)]
struct NewTask {
    request_id: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    job_id: Option<String>,
    from: String,
    to: String,
    #[serde(default)]
    requested_vehicle: Option<String>,
}
fn rcs_handler(s: &Shared<RcsState>, r: &Request) -> Response {
    if let Some(r) = common("rcs", r, Some(s)) {
        return r;
    }
    match (r.method.as_str(), r.path.as_str()) {
        ("GET", "/api/state") => {
            ok(rcs_state_json(s))
        }
        ("GET", "/api/vehicles") => ok(json!({"vehicles":fleet_states()})),
        ("POST", "/api/job-lock") => workflow::vehicle_lock(s,r,false),
        ("POST", "/api/job-unlock") => workflow::vehicle_lock(s,r,true),
        ("POST", "/api/job-cancel") => workflow::cancel_barrier(s,r),
        ("POST", "/api/job-recover") => workflow::recovery::rcs(s,r),
        ("POST", "/api/job-continue-check") => workflow::continuation::rcs_check(s,r),
        ("POST", "/api/entry") => traffic::entry(s, r),
        ("POST", "/api/roads/recover") => navigation::recover(s, r),
        ("POST", "/api/sim/park") => traffic::park(s, r),
        ("POST", "/api/sim/blocking-demo") => traffic::demo(s),
        ("POST", "/api/tasks") => {
            let n: NewTask = match parse(r) {
                Ok(n) => n,
                Err(e) => return e,
            };
            if !matches!(n.kind.as_str(), "TRANSPORT" | "MOVE") {
                return error(400, "任務類型僅接受 TRANSPORT 搬運或 MOVE 點到點移動");
            }
            if n.requested_vehicle
                .as_deref()
                .is_some_and(|id| vehicle_port(id).is_none())
            {
                return error(400, "未知的指定車號");
            }
            if n.request_id.is_empty()
                || n.request_id.len() > 100
                || node(&n.from).is_none()
                || node(&n.to).is_none()
                || n.from == n.to
            {
                return error(400, "請填有效請求編號，並選擇不同的起點與終點。");
            }
            let mut s = s.lock().unwrap();
            if let Some(job)=n.job_id.as_deref() {
                if workflow::canceled(&s,job) {return error(409,"整張作業正在取消，拒絕新增子任務");}
                if r.source != "wcs" || n.kind != "MOVE" || !s.job_locks.iter().any(|l|
                    l.job_id == job && n.requested_vehicle.as_deref() == Some(&l.vehicle_id)) {
                    return error(409,"WCS 子任務必須使用該作業已鎖定的車輛");
                }
            }
            if let Some(t) = s.tasks.iter().find(|t| t.request_id == n.request_id) {
                if t.from != n.from || t.to != n.to || t.requested_vehicle != n.requested_vehicle || t.kind != n.kind || t.job_id != n.job_id {
                    return error(409, "相同請求編號已用於另一條路線。");
                };
                return ok(json!({"task":t,"duplicate":true}));
            }
            if s.storage_error.is_some() {
                return error(503, "任務儲存異常，暫停接單。");
            }
            if s.tasks.len() >= 10000 {
                return error(409, "模擬任務已達 10000 筆上限。");
            }
            let ts = now();
            let t = Task {
                kind: n.kind,
                job_id: n.job_id,
                parent_task: None,
                id: format!("CT-{ts}-{}", s.tasks.len() + 1),
                request_id: n.request_id,
                from: n.from,
                to: n.to,
                status: "QUEUED".into(),
                progress: 0,
                phase: "等待派車".into(),
                created_at: ts,
                updated_at: ts,
                note: String::new(),
                last_seq: 0,
                assigned_vehicle: None,
                requested_vehicle: n.requested_vehicle,
                wait_reason: None,
                wait_since: None,
                timing: TaskTiming::default(),
                routes: vec![],
            };
            s.tasks.push(t.clone());
            log_event(&mut s, &t.id, "介接服務已建立任務，進入 RCS 佇列");
            save(&mut s);
            if s.storage_error.is_some() {
                return error(503, "任務儲存失敗；請用相同請求編號重試，勿另建任務。");
            }
            response(201, json!({"task":t,"duplicate":false}))
        }
        ("POST", "/api/dispatch") => {
            let v: Value = match parse(r) {
                Ok(v) => v,
                Err(e) => return e,
            };
            let Some(enabled) = v["enabled"].as_bool() else {
                return error(400, "enabled 必須是布林值");
            };
            let mut s = s.lock().unwrap();
            s.auto_dispatch = enabled;
            log_event(
                &mut s,
                "SYSTEM",
                if enabled {
                    "開啟自動派車"
                } else {
                    "暫停新任務派發；執行中的任務繼續"
                },
            );
            save(&mut s);
            ok(json!({"auto_dispatch":enabled}))
        }
        ("POST", "/api/events") => {
            let c: Callback = match parse(r) {
                Ok(v) => v,
                Err(e) => return e,
            };
            if !matches!(c.status.as_str(), "RUNNING" | "COMPLETED" | "CANCELED")
                || c.progress > 100
            {
                return error(400, "無效的回報狀態");
            }
            let mut s = s.lock().unwrap();
            let Some(i) = s.tasks.iter().position(|t| t.id == c.task_id) else {
                return error(404, "找不到任務");
            };
            if s.tasks[i].assigned_vehicle.as_deref() != Some(c.vehicle_id.as_str()) {
                return error(409, "回報車號與任務指派車號不符");
            }
            if terminal(&s.tasks[i].status) || c.seq <= s.tasks[i].last_seq {
                return ok(json!({"ok":true,"ignored":true}));
            }
            let changed = s.tasks[i].phase != c.phase || s.tasks[i].status != c.status;
            let t = &mut s.tasks[i];
            t.last_seq = c.seq;
            t.status = c.status.clone();
            t.progress = c.progress;
            t.phase = c.phase.clone();
            t.updated_at = now();
            t.timing.accepted_at.get_or_insert(t.updated_at);
            t.timing.first_progress_at.get_or_insert(t.updated_at);
            if c.status == "COMPLETED" {
                t.timing.completed_at = Some(t.updated_at);
            }
            if c.status == "CANCELED" {
                t.timing.canceled_at = Some(t.updated_at);
            }
            if terminal(&c.status) {
                s.tasks[i].wait_reason = None;
                s.tasks[i].wait_since = None;
                // Retain the lease until fresh telemetry confirms the final position.
                // Otherwise a stale position snapshot could briefly admit another car.
            }
            if changed {
                log_event(
                    &mut s,
                    &c.task_id,
                    &format!("{} 回報：{}", c.vehicle_id, c.phase),
                );
            }
            save(&mut s);
            if s.storage_error.is_some() {
                return error(503, "回報已接收，但儲存失敗");
            };
            ok(json!({"ok":true}))
        }
        _ if r.method == "POST" && r.path.starts_with("/api/cancel/") => {
            let id = r.path.trim_start_matches("/api/cancel/");
            let assigned_port;
            {
                let mut s = s.lock().unwrap();
                let Some(i) = s.tasks.iter().position(|t| t.id == id) else {
                    return error(404, "找不到任務");
                };
                let cancel_uncertain=s.tasks[i].status=="INTERRUPTED"
                    && s.tasks[i].job_id.as_deref().is_some_and(|job|workflow::canceled(&s,job));
                if terminal(&s.tasks[i].status) && !cancel_uncertain {
                    return error(409, "任務已結束");
                };
                if s.tasks[i].status == "QUEUED" {
                    s.tasks[i].status = "CANCELED".into();
                    s.tasks[i].phase = "已取消排隊".into();
                    s.tasks[i].updated_at = now();
                    s.tasks[i].timing.canceled_at = Some(s.tasks[i].updated_at);
                    log_event(&mut s, id, "取消佇列中的任務");
                    save(&mut s);
                    return ok(json!({"ok":true}));
                }
                if s.tasks[i].status == "DISPATCHING" {
                    return error(409, "正在交付車端，請稍後再取消");
                }
                assigned_port = match s.tasks[i]
                    .assigned_vehicle
                    .as_deref()
                    .and_then(vehicle_port)
                {
                    Some(p) => p,
                    None => return error(409, "此任務沒有可確認的指派車輛"),
                };
            }
            match http(
                assigned_port,
                "POST",
                &format!("/api/cancel/{id}"),
                json!({}),
            ) {
                Ok(v) => ok(v),
                Err(e) => error(502, &format!("尚未確認車端取消：{e}")),
            }
        }
        _ => error(404, "找不到 API"),
    }
}
fn vehicle_json(v: &Vehicle) -> Value {
    json!({"role":"agv","vehicle_id":vehicle_id(),"port":vehicle_port(vehicle_id()),"online":true,"mode":"SIMULATION","x":v.x,"y":v.y,"battery":85,"paused":v.paused,"active":v.active.as_ref().map(|a|&a.mission),"phase":v.phase,"progress":v.progress,"last_task":v.last_task,"pending_callbacks":v.pending.len(),"sample_at":now(),"callback_error":v.callback_error,"waiting_for":v.waiting_for,"station":traffic::location(&json!({"x":v.x,"y":v.y})),"current_node":navigation::at(v.x,v.y),"route_path":v.route_path,"route_token":v.route_token,"road_edge":v.road_edge})
}
fn agv_handler(s: &Shared<Vehicle>, r: &Request) -> Response {
    if r.method == "GET" && r.path == "/api/snapshot" {
        let vehicle = vehicle_json(&s.lock().unwrap());
        return ok(json!({"role":"agv","rcs":null,"rcs_error":null,
            "vehicle":vehicle,"vehicle_error":null,"vehicles":[vehicle],
            "traffic":trace_store().lock().unwrap().iter().cloned().collect::<Vec<_>>()}));
    }
    if let Some(r) = common("agv", r, None) {
        return r;
    }
    match (r.method.as_str(), r.path.as_str()) {
        ("GET", "/api/state") => ok(vehicle_json(&s.lock().unwrap())),
        ("POST", "/api/sim/position") => {
            if r.source != "rcs" {
                return error(403, "模擬位置只能由 RCS 設定");
            }
            let p: Value = match parse(r) {
                Ok(v) => v,
                Err(e) => return e,
            };
            let Some((x, y)) = p["node"].as_str().and_then(node) else {
                return error(400, "未知點位");
            };
            let mut v = s.lock().unwrap();
            if v.active.is_some() || !v.pending.is_empty() {
                return error(409, "車輛有任務或回報尚未送完");
            }
            v.route_path.clear();
            v.route_token = None;
            v.road_edge = None;
            v.x = x;
            v.y = y;
            v.waiting_for = None;
            v.progress = 0;
            v.phase = "模擬設定位置，待命中".into();
            ok(vehicle_json(&v))
        }
        ("POST", "/api/missions") => {
            let m: Mission = match parse(r) {
                Ok(m) => m,
                Err(e) => return e,
            };
            if !matches!(m.kind.as_str(), "TRANSPORT" | "MOVE" | "YIELD") {
                return error(400, "未知任務類型");
            }
            if m.kind == "YIELD"
                && (m.parent_task.is_none() || !PARKING_NODES.contains(&m.to.as_str()))
            {
                return error(400, "讓位任務需有原任務編號及可用避讓／空閒點");
            }
            if m.vehicle_id != vehicle_id() {
                return error(409, "任務指定車號與此車端不符");
            }
            if node(&m.from).is_none()
                || node(&m.to).is_none()
                || m.from == m.to
                || m.task_id.is_empty()
            {
                return error(400, "無效的模擬任務");
            };
            let mut v = s.lock().unwrap();
            if v.seen.contains(&m.task_id) {
                return ok(json!({"accepted":true,"duplicate":true,"vehicle_id":vehicle_id()}));
            }
            if v.active.is_some() || v.paused || !v.pending.is_empty() {
                return error(409, "車輛忙碌、暫停或仍有回報待送");
            };
            v.seen.push_back(m.task_id.clone());
            while v.seen.len() > 1000 {
                v.seen.pop_front();
            }
            v.waiting_for = None;
            v.route_path.clear();
            v.route_token = None;
            let moving = m.kind == "MOVE";
            v.active = Some(Active {
                mission: m,
                ticks: 0,
                seq: 0,
                motion: None,
            });
            v.progress = 0;
            v.phase = if moving { "已接單，準備前往移動起點" } else { "已接單，準備前往取料點" }.into();
            ok(json!({"accepted":true,"vehicle_id":vehicle_id()}))
        }
        ("POST", "/api/control") => {
            let p: Value = match parse(r) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let Some(paused) = p["paused"].as_bool() else {
                return error(400, "paused 必須是布林值");
            };
            let mut v = s.lock().unwrap();
            v.paused = paused;
            ok(vehicle_json(&v))
        }
        _ if r.method == "POST" && r.path.starts_with("/api/cancel/") => {
            let id = r.path.trim_start_matches("/api/cancel/");
            let mut v = s.lock().unwrap();
            let Some(a) = v.active.as_ref() else {
                return error(409, "車輛沒有執行中的任務");
            };
            if a.mission.task_id != id {
                return error(409, "車輛正在執行另一張任務");
            };
            let c = Callback {
                task_id: id.into(),
                seq: a.seq + 1,
                status: "CANCELED".into(),
                progress: v.progress,
                phase: "模擬走行已取消".into(),
                vehicle_id: vehicle_id().into(),
            };
            v.last_task = Some(c.clone());
            v.pending.push_back(c);
            v.active = None;
            v.waiting_for = None;
            v.phase = "已取消，等待下一張任務".into();
            ok(json!({"ok":true}))
        }
        _ => error(404, "找不到 API"),
    }
}
fn integration_handler(r: &Request) -> Response {
    if let Some(r) = common("central", r, None) {
        return r;
    }
    match (r.method.as_str(), r.path.as_str()) {
        ("GET", "/api/state") | ("POST", "/api/tasks") => proxy(RCS, r),
        ("GET", "/api/vehicles") => proxy(RCS, r),
        ("GET", "/api/vehicle") => match http(RCS, "GET", "/api/vehicles", Value::Null) {
            Ok(v) => match v["vehicles"].as_array().and_then(|fleet| {
                fleet.iter().find(|vehicle| vehicle["vehicle_id"] == "CT-SIM-01")
            }) {
                Some(vehicle) if vehicle["online"] == true => ok(vehicle.clone()),
                Some(vehicle) => error(502, vehicle["error"].as_str().unwrap_or("AGV offline")),
                None => error(502, "RCS returned no CT-SIM-01 state"),
            },
            Err(e) => error(502, &e),
        },
        _ if r.method == "POST" && r.path.starts_with("/api/cancel/") => proxy(RCS, r),
        _ => error(404, "找不到 API"),
    }
}
fn dispatch_loop(s: Shared<RcsState>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(500));
        let fleet = fleet_states();
        {
            let mut state = s.lock().unwrap();
            state.traffic.refresh(&fleet);
            navigation::release_arrived(&mut state);
            yielding::cancel_orphaned(&mut state);
            let completed: Vec<String> = state
                .traffic
                .leases
                .iter()
                .filter(|l| {
                    state.tasks.iter().any(|t| {
                        t.id == l.task_id
                            && matches!(t.status.as_str(), "COMPLETED" | "CANCELED" | "FAILED")
                            && state.traffic.observations.iter().any(|v| {
                                v["vehicle_id"] == l.vehicle_id
                                    && v["online"] == true
                                    && v["active"].is_null()
                                    && v["pending_callbacks"] == 0
                                    && v["sample_at"].as_u64().unwrap_or(0) >= t.updated_at
                            })
                    })
                })
                .map(|l| l.task_id.clone())
                .collect();
            if !completed.is_empty() {
                state
                    .traffic
                    .leases
                    .retain(|l| !completed.contains(&l.task_id));
                save(&mut state);
            }
            if !state.auto_dispatch || state.storage_error.is_some() {
                continue;
            }
        }
        let mut available: Vec<Value> = fleet
            .into_iter()
            .filter(|v| {
                v["online"] == true
                    && v["active"].is_null()
                    && v["paused"] == false
                    && v["pending_callbacks"].as_u64() == Some(0)
            })
            .collect();
        loop {
            let assignment = {
                let mut state = s.lock().unwrap();
                if !state.auto_dispatch || state.storage_error.is_some() {
                    break;
                }
                available.retain(|v| {
                    !state
                        .roads
                        .leases
                        .iter()
                        .any(|l| Some(l.vehicle_id.as_str()) == v["vehicle_id"].as_str())
                        && !state.traffic.leases.iter().any(|l| {
                            Some(l.vehicle_id.as_str()) == v["vehicle_id"].as_str()
                                && !state.tasks.iter().any(|t| {
                                    t.id == l.task_id && t.kind == "YIELD" && t.status == "QUEUED"
                                })
                        })
                        && !state.tasks.iter().any(|t| {
                            matches!(t.status.as_str(), "RUNNING" | "DISPATCHING")
                                && t.assigned_vehicle.as_deref() == v["vehicle_id"].as_str()
                        })
                });
                if available.is_empty() {
                    break;
                }
                let Some(i) = state
                    .tasks
                    .iter()
                    .position(|t| {
                        t.status == "QUEUED"
                            && t.kind == "YIELD"
                            && available
                                .iter()
                                .any(|v| workflow::eligible(&state,t,v))
                    })
                    .or_else(|| {
                        state
                            .tasks
                            .iter()
                            .position(|t| t.status == "QUEUED" && t.kind != "YIELD"
                                && available.iter().any(|v| workflow::eligible(&state,t,v)))
                    })
                else {
                    break;
                };
                let eligible_positions: Vec<String> = available.iter()
                    .filter(|v| workflow::eligible(&state,&state.tasks[i],v))
                    .filter_map(|v|v["vehicle_id"].as_str().map(str::to_owned)).collect();
                available.retain(|v| v["vehicle_id"].as_str().is_some_and(|id|eligible_positions.iter().any(|x|x==id)));
                if let Some(id) = state.tasks[i].requested_vehicle.as_deref() {
                    if let Some(pos) = available.iter().position(|v| v["vehicle_id"] == id) {
                        available.swap(0, pos);
                    } else {
                        break;
                    }
                }
                let pickup = node(&state.tasks[i].from).unwrap();
                let distance = |v: &Value| {
                    let x = v["x"].as_f64().unwrap_or(0.);
                    let y = v["y"].as_f64().unwrap_or(0.);
                    (x - pickup.0).powi(2) + (y - pickup.1).powi(2)
                };
                if state.tasks[i].requested_vehicle.is_none() {
                    available.sort_by(|a, b| {
                        distance(a)
                            .partial_cmp(&distance(b))
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| a["vehicle_id"].as_str().cmp(&b["vehicle_id"].as_str()))
                    });
                }
                let car = available.remove(0);
                let id = car["vehicle_id"].as_str().unwrap().to_string();
                let port = vehicle_port(&id).unwrap();
                let t = &mut state.tasks[i];
                t.assigned_vehicle = Some(id.clone());
                t.status = "DISPATCHING".into();
                t.phase = format!("正在交付 {id}");
                t.updated_at = now();
                t.timing.dispatched_at = Some(t.updated_at);
                let t = t.clone();
                log_event(
                    &mut state,
                    &t.id,
                    &format!(
                        "RCS 選擇最近空閒車 {id}（示意距離 {:.1}），交付車端",
                        distance(&car).sqrt()
                    ),
                );
                save(&mut state);
                if state.storage_error.is_some() {
                    break;
                }
                (t, id, port)
            };
            let (task, id, port) = assignment;
            let result = http(
                port,
                "POST",
                "/api/missions",
                json!({"task_id":task.id,"vehicle_id":id,"from":task.from,"to":task.to,"kind":task.kind,"parent_task":task.parent_task}),
            );
            let mut state = s.lock().unwrap();
            let Some(i) = state.tasks.iter().position(|t| t.id == task.id) else {
                continue;
            };
            match result {
                Ok(_) => {
                    if state.tasks[i].status == "DISPATCHING" {
                        state.tasks[i].status = "RUNNING".into();
                        state.tasks[i].phase = format!("{id} 已接單");
                        state.tasks[i].timing.accepted_at = Some(now());
                        state.tasks[i].updated_at = now();
                        log_event(&mut state, &task.id, &format!("{id} 已接受任務"));
                    }
                }
                Err(e) => {
                    if state.tasks[i].status == "DISPATCHING" {
                        state.tasks[i].status = "INTERRUPTED".into();
                        state.tasks[i].phase = "交付結果待確認".into();
                        state.tasks[i].timing.interrupted_at = Some(now());
                        state.tasks[i].updated_at = now();
                        state.tasks[i].note = format!("{id}: {e}；不自動改派，請確認車端狀態。");
                        log_event(
                            &mut state,
                            &task.id,
                            &format!("{id} 的任務交付未確認，不自動重派"),
                        );
                    }
                }
            }
            save(&mut state);
        }
    });
}
fn vehicle_loop(s: Shared<Vehicle>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(500));
        let entry_request = {
            let v = s.lock().unwrap();
            v.active
                .as_ref()
                .filter(|a| {
                    !v.paused
                        && a.motion.is_none()
                        && (a.ticks == 0 || (a.ticks == 12 && a.mission.kind != "YIELD"))
                })
                .map(|a| (a.mission.task_id.clone(), a.ticks, a.mission.kind.clone()))
        };
        let mut admission = true;
        if let Some((id, ticks, kind)) = entry_request {
            let result = http(
                RCS,
                "POST",
                "/api/entry",
                json!({"task_id":id,"vehicle_id":vehicle_id(),"stage":if kind=="YIELD" {"move"}else if ticks==0 {"pickup"}else{"destination"}}),
            );
            let mut v = s.lock().unwrap();
            if v.active
                .as_ref()
                .is_some_and(|a| a.mission.task_id == id && a.ticks == ticks)
            {
                match result {
                    Ok(value) if value["granted"] == true => {
                        let path: Vec<String> = value["path"]
                            .as_array()
                            .map(|p| {
                                p.iter()
                                    .filter_map(|n| n.as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if path.first().and_then(|id| navigation::xy(id)) != Some((v.x, v.y)) {
                            admission = false;
                            v.waiting_for = Some("路徑起點不符，等待確認".into());
                        } else if let Some(motion) = navigation::Motion::new(path.clone()) {
                            v.active.as_mut().unwrap().motion = Some(motion);
                            v.route_path = path;
                            v.route_token = value["route_token"].as_str().map(str::to_owned);
                            v.waiting_for = None;
                        } else {
                            admission = false;
                            v.waiting_for = Some("收到無效路徑，保持原位".into());
                        }
                    }
                    other => {
                        admission = false;
                        let reason = match other {
                            Ok(value) => value["reason"]
                                .as_str()
                                .unwrap_or("等待進站許可")
                                .to_string(),
                            Err(e) => format!("等待 RCS 進站許可：{e}"),
                        };
                        v.phase = reason.clone();
                        v.waiting_for = Some(reason);
                    }
                }
            } else {
                admission = false;
            }
        }
        let pending = {
            let mut v = s.lock().unwrap();
            if !v.paused && admission {
                navigation::advance(&mut v);
            }
            v.pending.front().cloned()
        };
        if let Some(c) = pending {
            let result = http(
                RCS,
                "POST",
                "/api/events",
                serde_json::to_value(&c).unwrap(),
            );
            let mut v = s.lock().unwrap();
            match result {
                Ok(_) => {
                    v.pending.pop_front();
                    v.callback_error = None
                }
                Err(e) => v.callback_error = Some(e),
            }
        }
    });
}

fn serve(
    role: String,
    port: u16,
    rcs: Shared<RcsState>,
    agv: Shared<Vehicle>,
) -> Result<(), String> {
    let listener =
        TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("Cannot bind {port}: {e}"))?;
    println!("{} ready: http://127.0.0.1:{port} (SIMULATION ONLY)", role);
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        let role = role.clone();
        let rcs = rcs.clone();
        let agv = agv.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let request = read_request(&mut s);
            let received = now();
            let r = match &request {
                Ok(r) => match role.as_str() {
                    "central" => integration_handler(&r),
                    "rcs" => rcs_handler(&rcs, &r),
                    "wcs"|"ecs" => workflow::handler(&role,&r),
                    _ => agv_handler(&agv, &r),
                },
                Err(e) => error(400, e),
            };
            let response_ready_at = now();
            let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
            let written=write!(s,"HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",r.code,if r.code<400{"OK"}else{"Error"},r.content_type,r.body.len()).and_then(|_|s.write_all(&r.body));
            let responded_at = written.as_ref().ok().map(|_| now());
            if let Ok(req) = request {
                record_trace(
                    &role,
                    port,
                    &req,
                    &r,
                    received,
                    response_ready_at,
                    responded_at,
                    started.elapsed().as_secs_f64() * 1000.,
                    written.err().map(|e| e.to_string()),
                );
            }
        });
    }
    Ok(())
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let role = args
        .windows(2)
        .find(|w| w[0] == "--role")
        .map(|w| w[1].as_str())
        .unwrap_or("");
    if !matches!(role, "central" | "rcs" | "agv" | "wcs" | "ecs") {
        eprintln!("Usage: rust-rcs-lab --role central|rcs|agv|wcs|ecs");
        std::process::exit(2)
    }
    let id = args
        .windows(2)
        .find(|w| w[0] == "--vehicle-id")
        .map(|w| w[1].as_str())
        .unwrap_or("CT-SIM-01");
    if role == "agv" && vehicle_port(id).is_none() {
        eprintln!("Unknown vehicle id: {id}");
        std::process::exit(2)
    }
    let _ = VEHICLE_ID.set(id.into());
    let _ = ROLE.set(if role == "agv" {
        id.into()
    } else {
        role.into()
    });
    if let Err(e)=workflow::start(role) {eprintln!("{e}");std::process::exit(1);}
    let state = if role == "rcs" {
        match load() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1)
            }
        }
    } else {
        RcsState::default()
    };
    let rcs = Arc::new(Mutex::new(state));
    let agv = Arc::new(Mutex::new(Vehicle::default()));
    if role == "rcs" {
        dispatch_loop(rcs.clone());
    }
    if role == "agv" {
        vehicle_loop(agv.clone());
    }
    let port = match role {
        "central" => INTEGRATION,
        "rcs" => RCS,
        "wcs" => workflow::WCS,
        "ecs" => workflow::ECS,
        _ => vehicle_port(vehicle_id()).unwrap(),
    };
    if let Err(e) = serve(role.into(), port, rcs, agv) {
        eprintln!("{e}");
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn req(method: &str, path: &str, body: Value) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            body: serde_json::to_vec(&body).unwrap(),
            source: "test".into(),
        }
    }
    #[test]
    fn duplicate_mission_never_restarts() {
        let v = Arc::new(Mutex::new(Vehicle::default()));
        let r = req(
            "POST",
            "/api/missions",
            json!({"task_id":"T1","from":"A","to":"B"}),
        );
        assert_eq!(agv_handler(&v, &r).code, 200);
        let mut s = v.lock().unwrap();
        s.active.as_mut().unwrap().ticks = 7;
        drop(s);
        assert_eq!(agv_handler(&v, &r).code, 200);
        assert_eq!(v.lock().unwrap().active.as_ref().unwrap().ticks, 7);
    }
    #[test]
    fn busy_vehicle_rejects_second_task() {
        let v = Arc::new(Mutex::new(Vehicle::default()));
        for (id, code) in [("T1", 200), ("T2", 409)] {
            assert_eq!(
                agv_handler(
                    &v,
                    &req(
                        "POST",
                        "/api/missions",
                        json!({"task_id":id,"from":"A","to":"B"})
                    )
                )
                .code,
                code
            );
        }
    }
    #[test]
    fn wrong_cancel_does_not_stop_other_mission() {
        let v = Arc::new(Mutex::new(Vehicle::default()));
        agv_handler(
            &v,
            &req(
                "POST",
                "/api/missions",
                json!({"task_id":"T1","from":"A","to":"B"}),
            ),
        );
        assert_eq!(
            agv_handler(&v, &req("POST", "/api/cancel/T2", json!({}))).code,
            409
        );
        assert!(v.lock().unwrap().active.is_some());
    }
    #[test]
    fn cancel_produces_terminal_callback() {
        let v = Arc::new(Mutex::new(Vehicle::default()));
        agv_handler(
            &v,
            &req(
                "POST",
                "/api/missions",
                json!({"task_id":"T1","from":"A","to":"B"}),
            ),
        );
        assert_eq!(
            agv_handler(&v, &req("POST", "/api/cancel/T1", json!({}))).code,
            200
        );
        let s = v.lock().unwrap();
        assert!(s.active.is_none());
        assert_eq!(s.pending.front().unwrap().status, "CANCELED");
    }
}
