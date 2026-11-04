//! First-stage workflow orchestration and simulated equipment handshakes.
use super::*;
mod cancel;
mod limits;
mod attention;
pub mod continuation;
pub mod recovery;

pub const WCS: u16 = 8215;
pub const ECS: u16 = 8216;
static JOBS: OnceLock<Shared<Jobs>> = OnceLock::new();
static EQUIPMENT: OnceLock<Shared<Equipment>> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize)]
pub struct VehicleLock {
    pub job_id: String,
    pub vehicle_id: String,
    pub at: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct JobCancellation {
    pub job_id: String,
    pub at: u64,
    pub released_at: Option<u64>,
}
pub fn canceled(state: &RcsState, job: &str) -> bool {
    state.job_cancellations.iter().any(|item|item.job_id==job)
}
pub fn cancel_barrier(s: &Shared<RcsState>, r: &Request) -> Response {
    if r.source!="wcs" {return error(403,"僅供 WCS 取消協調");}
    let v:Value=match parse(r){Ok(v)=>v,Err(e)=>return e};
    let Some(job)=v["job_id"].as_str().filter(|id|!id.is_empty()&&id.len()<=120)else{return error(400,"job_id 必填");};
    let mut s=s.lock().unwrap();
    if s.storage_error.is_some(){return error(503,"RCS 儲存異常，取消屏障尚未確認");}
    if let Some(fence)=s.job_cancellations.iter().find(|f|f.job_id==job){return ok(json!({"job_id":job,"fence":fence,"duplicate":true}));}
    if s.job_cancellations.len()>=10000{return error(409,"取消紀錄達上限");}
    s.job_cancellations.push(JobCancellation{job_id:job.into(),at:now(),released_at:None});save(&mut s);
    if s.storage_error.is_some(){return error(503,"取消屏障保存失敗");}
    ok(json!({"job_id":job,"fenced":true}))
}

pub fn eligible(state: &RcsState, task: &Task, car: &Value) -> bool {
    !task.job_id.as_deref().is_some_and(|job|canceled(state,job))
        && task.requested_vehicle.as_deref().is_none_or(|id| car["vehicle_id"] == id)
        && state.job_locks.iter().all(|lock| {
            car["vehicle_id"] != lock.vehicle_id || task.job_id.as_deref() == Some(&lock.job_id)
        })
}

pub fn vehicle_lock(s: &Shared<RcsState>, r: &Request, release: bool) -> Response {
    if r.source != "wcs" {return error(403,"僅供 WCS 作業協調使用（本機來源標頭非正式認證）");}
    let value: Value = match parse(r) { Ok(v) => v, Err(e) => return e };
    let Some(job) = value["job_id"].as_str().filter(|id| !id.is_empty() && id.len() <= 120) else {
        return error(400, "job_id 必填");
    };
    let fleet = fleet_states();
    let mut state = s.lock().unwrap();
    if state.storage_error.is_some() { return error(503, "RCS 儲存異常"); }
    if release {
        let Some(lock) = state.job_locks.iter().find(|l| l.job_id == job).cloned() else {
            return ok(json!({"released":true,"duplicate":true}));
        };
        let safe = fleet.iter().any(|v| v["vehicle_id"] == lock.vehicle_id && v["online"] == true
            && v["active"].is_null() && v["pending_callbacks"] == 0)
            && !state.roads.leases.iter().any(|l| l.vehicle_id == lock.vehicle_id)
            && !state.traffic.leases.iter().any(|l| l.vehicle_id == lock.vehicle_id)
            && !state.tasks.iter().any(|t| t.job_id.as_deref() == Some(job)
                && !matches!(t.status.as_str(), "COMPLETED" | "CANCELED" | "FAILED"));
        if !safe { return error(409, "尚有任務、回報或未確認道路預約，保留作業用車"); }
        state.job_locks.retain(|l| l.job_id != job);
        if let Some(fence)=state.job_cancellations.iter_mut().find(|f|f.job_id==job){fence.released_at.get_or_insert(now());}
        save(&mut state);
        if state.storage_error.is_some() { return error(503, "解除鎖定儲存失敗"); }
        return ok(json!({"released":true}));
    }
    if canceled(&state,job){return error(409,"作業正在取消，不再鎖定新車輛");}
    if let Some(lock) = state.job_locks.iter().find(|l| l.job_id == job) {
        return ok(json!({"vehicle_id":lock.vehicle_id,"duplicate":true}));
    }
    let from = value["from"].as_str().and_then(node);
    let Some(from) = from else { return error(400, "無效起點"); };
    if value["requested_vehicle"].as_str().is_some_and(|id| vehicle_port(id).is_none()) {
        return error(400, "未知車號");
    }
    let mut available: Vec<&Value> = fleet.iter().filter(|v| {
        let id = v["vehicle_id"].as_str().unwrap_or("");
        v["online"] == true && v["active"].is_null() && v["paused"] == false && v["pending_callbacks"] == 0
            && value["requested_vehicle"].as_str().is_none_or(|requested| requested == id)
            && !state.job_locks.iter().any(|l| l.vehicle_id == id)
            && !state.roads.leases.iter().any(|l| l.vehicle_id == id)
            && !state.traffic.leases.iter().any(|l| l.vehicle_id == id)
            && !state.tasks.iter().any(|t| t.assigned_vehicle.as_deref() == Some(id)
                && matches!(t.status.as_str(), "RUNNING" | "DISPATCHING"))
    }).collect();
    available.sort_by(|a,b| {
        let distance = |v: &Value| (v["x"].as_f64().unwrap_or(0.)-from.0).powi(2)
            + (v["y"].as_f64().unwrap_or(0.)-from.1).powi(2);
        distance(a).partial_cmp(&distance(b)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let Some(car) = available.first() else { return error(409, "等待可用且未被其他作業鎖定的車輛"); };
    let id = car["vehicle_id"].as_str().unwrap().to_string();
    state.job_locks.push(VehicleLock { job_id:job.into(),vehicle_id:id.clone(),at:now() });
    save(&mut state);
    if state.storage_error.is_some() { return error(503, "作業用車鎖定儲存失敗"); }
    ok(json!({"vehicle_id":id,"duplicate":false}))
}

#[derive(Clone, Serialize, Deserialize)]
struct Job {
    id: String, request_id: String, from: String, to: String,
    requested_vehicle: Option<String>, vehicle_id: Option<String>,
    status: String, step: String, paused: bool, loaded: bool,
    created_at: u64, updated_at: u64, wait_reason: Option<String>,
    task_ids: Vec<String>, events: Vec<Value>,
    #[serde(default)] cancel_requested_at: Option<u64>,
    #[serde(default)] cancel_deadline_at: Option<u64>,
    #[serde(default)] canceled_at: Option<u64>,
    #[serde(default)] cancel_reason: Option<String>,
    #[serde(default)] cancel_phase: Option<String>,
    #[serde(default)] cancel_requires_review: bool,
    #[serde(default)] cancel_evidence: Value,
    #[serde(default)] cancel_verify_since: Option<u64>,
    #[serde(default)] recovery: Option<recovery::Recovery>,
    #[serde(default)] continuations: Vec<continuation::Attempt>,
    #[serde(default)] timeout_policy: limits::WcsPolicy,
    #[serde(default)] wait_started_at: Option<u64>,
    #[serde(default)] wait_deadline_at: Option<u64>,
    #[serde(default)] alarm: Option<limits::Alarm>,
    #[serde(default)] attention_since: Option<u64>,
}
#[derive(Default, Serialize, Deserialize)]
struct Jobs { #[serde(default)] attention_policy: attention::Policy, #[serde(default)] policy: limits::WcsPolicy, jobs: Vec<Job>, #[serde(skip)] storage_error: Option<String> }
#[derive(Clone, Serialize, Deserialize)]
struct Device {
    id: String, material_ready: bool, can_receive: bool, fault: bool, auto_complete: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct Interaction {
    command_id: String, job_id: String, vehicle_id: String, station: String, action: String,
    status: String, requested_at: u64, allowed_at: Option<u64>, completed_at: Option<u64>,
    phase: String,
    #[serde(default)] canceled_at: Option<u64>,
    #[serde(default)] timeout_policy: limits::EcsPolicy,
    #[serde(default)] allow_deadline_at: Option<u64>,
    #[serde(default)] completion_deadline_at: Option<u64>,
    #[serde(default)] alarm: Option<limits::Alarm>,
}
#[derive(Default, Serialize, Deserialize)]
struct Equipment {
    #[serde(default)] policy: limits::EcsPolicy,
    #[serde(default)] reconciliations: Vec<recovery::Receipt>,
    #[serde(default)] confirmed_results: Vec<continuation::Review>,
    #[serde(default)] canceled_jobs: Vec<String>,
    devices: Vec<Device>, interactions: Vec<Interaction>, #[serde(skip)] storage_error: Option<String>,
}
fn path(name: &str) -> PathBuf { db_path().with_file_name(name) }
fn equipment_station(id: &str) -> bool {
    node(id).is_some() && matches!(navigation::station_meta(id).1,"machine"|"warehouse")
}
fn persist<T: Serialize>(name: &str, state: &T) -> Result<(), String> {
    let p=path(name);
    fs::create_dir_all(p.parent().unwrap()).map_err(|e|e.to_string())?;
    let temporary=p.with_extension("tmp");
    fs::write(&temporary,serde_json::to_vec_pretty(state).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    fs::rename(temporary,p).map_err(|e|e.to_string())
}
fn save_jobs(state: &mut Jobs) { state.storage_error=persist("wcs.json",state).err(); }
fn save_equipment(state: &mut Equipment) { state.storage_error=persist("ecs.json",state).err(); }
fn read<T: for<'a> Deserialize<'a> + Default>(name: &str) -> Result<T,String> {
    match fs::read(path(name)) {
        Ok(bytes)=>serde_json::from_slice(&bytes).map_err(|e|format!("{name}: {e}")),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(T::default()),
        Err(e)=>Err(e.to_string()),
    }
}
fn change(job: &mut Job, step: &str, reason: &str) {
    job.step=step.into(); job.status=if step=="DONE" {"COMPLETED"} else {"RUNNING"}.into();
    job.wait_reason=None; job.updated_at=now();
    limits::enter(job,step);
    job.events.push(json!({"at":now(),"step":step,"message":reason}));
}
fn wait(job: &mut Job, reason: String) { job.status="WAITING".into();job.wait_reason=Some(reason);job.updated_at=now(); }
fn interrupt(job: &mut Job, reason: &str) {
    job.status="INTERRUPTED".into();job.wait_reason=Some(reason.into());job.updated_at=now();
    job.events.push(json!({"at":now(),"step":job.step,"message":reason}));
}

pub fn start(role: &str) -> Result<(),String> {
    if role=="wcs" {
        let mut state: Jobs=read("wcs.json")?;
        for job in &mut state.jobs {
            if job.status=="RESUMING" {
                job.paused=false;job.events.push(json!({"at":now(),"step":job.step,"message":"WCS 重啟，繼續核對人工接續要求與交接收據，不重做取放料"}));
            } else if job.status=="RECOVERING" {
                job.paused=false;
                job.events.push(json!({"at":now(),"step":job.step,"message":"WCS 重啟，繼續核對人工復歸收據，不恢復原搬運作業"}));
            } else if job.cancel_requested_at.is_some() && job.status=="CANCELING" {
                job.paused=false;
                job.events.push(json!({"at":now(),"step":job.step,"message":"WCS 重啟，繼續確認取消結果，不恢復正常流程"}));
            } else if !matches!(job.status.as_str(),"COMPLETED"|"CANCELED"|"INTERRUPTED") && job.vehicle_id.is_some() {
                interrupt(job,"WCS 已重啟：保留用車與進度，需人工確認，不重做取放料");
            }
        }
        for job in &mut state.jobs{attention::track(job);}
        save_jobs(&mut state);
        if let Some(e)=&state.storage_error {return Err(e.clone());}
        let state=Arc::new(Mutex::new(state)); let _=JOBS.set(state.clone());
        thread::spawn(move||loop {
            thread::sleep(Duration::from_millis(500));
            let jobs={let s=state.lock().unwrap(); if s.storage_error.is_some(){continue;} s.jobs.clone()};
            for mut job in jobs {
                if matches!(job.status.as_str(),"COMPLETED"|"CANCELED"|"INTERRUPTED"){continue;}
                if job.status=="RECOVERING"{recovery::advance(&mut job);}
                else if job.cancel_requested_at.is_some(){cancel::advance(&mut job);}
                else if job.status=="RESUMING"{continuation::advance(&mut job);}
                else {limits::monitor(&mut job);if job.status!="INTERRUPTED"&&!job.paused{step(&mut job);}}
                let mut s=state.lock().unwrap();
                if let Some(current)=s.jobs.iter_mut().find(|j|j.id==job.id) {
                    // A cancel arriving during an HTTP operation must survive
                    // the worker's older snapshot; its next turn only cancels.
                    if current.cancel_requested_at.is_some() && job.cancel_requested_at.is_none() {
                        job.attention_since=job.attention_since.or(current.attention_since);
                        job.cancel_requested_at=current.cancel_requested_at;
                        job.cancel_deadline_at=current.cancel_deadline_at;
                        job.cancel_reason=current.cancel_reason.clone();
                        job.cancel_requires_review=current.cancel_requires_review||job.status=="INTERRUPTED";
                        job.events.extend(current.events.iter().filter(|e|e["kind"]=="CANCEL_REQUESTED").cloned());
                        if job.status=="COMPLETED" {
                            job.cancel_phase=Some("TOO_LATE".into());
                            job.wait_reason=Some("取消要求到達時，整張作業已完成，未撤銷已完成的交接".into());
                            job.events.push(json!({"at":now(),"step":job.step,"message":"取消未撤銷已完成的作業，完成結果保留"}));
                        } else {
                            job.cancel_phase=Some("REQUESTED".into());job.status="CANCELING".into();job.wait_reason=Some("已收到取消，開始協調停止與確認結果".into());
                        }
                        job.events.sort_by_key(|event|event["at"].as_u64().unwrap_or(0));
                    }
                    attention::track(&mut job);
                    job.paused=if job.cancel_requested_at.is_some(){false}else{current.paused}; *current=job;
                }
                save_jobs(&mut s);
                if s.storage_error.is_some(){break;}
            }
        });
    } else if role=="ecs" {
        let mut state: Equipment=read("ecs.json")?;
        if state.devices.is_empty(){state.devices=NODES.iter().filter(|n|equipment_station(n.0)).map(|n|Device{id:n.0.into(),material_ready:true,can_receive:true,fault:false,auto_complete:true}).collect();}
        state.devices.retain(|d|equipment_station(&d.id));
        for op in &mut state.interactions {
            if matches!(op.status.as_str(),"REQUESTED"|"ALLOWED") {limits::fail_interaction(op,"ECS_RESTART","ECS 重啟，交接結果待確認","INTERRUPTED");}
        }
        save_equipment(&mut state);
        if let Some(e)=&state.storage_error {return Err(e.clone());}
        let state=Arc::new(Mutex::new(state));let _=EQUIPMENT.set(state.clone());
        thread::spawn(move||loop {
            thread::sleep(Duration::from_millis(250));let mut s=state.lock().unwrap();
            if s.storage_error.is_some(){continue;}
            let changed=limits::ecs_tick(&mut s);
            if changed {save_equipment(&mut s);}
        });
    }
    Ok(())
}

fn device_ready(job: &Job, station: &str, pick: bool) -> Result<bool,String> {
    let state=http(ECS,"GET","/api/equipment",Value::Null)?;
    if !state["storage_error"].is_null(){return Err("ECS 儲存異常，暫停前進".into());}
    let device=state["devices"].as_array().and_then(|ds|ds.iter().find(|d|d["id"]==station)).ok_or("找不到機台")?;
    let ready=device["fault"]==false && device[if pick {"material_ready"}else{"can_receive"}]==true;
    let _=job;
    Ok(ready)
}
fn step(job: &mut Job) {
    let result=(||->Result<(),String>{
        match job.step.as_str() {
            "WAIT_SOURCE" => {
                if device_ready(job,&job.from,true)? {change(job,"RESERVE","來源設備有料且無故障");}
                else {wait(job,format!("等待 {} 有料／解除故障",job.from));}
            }
            "RESERVE" => {
                let v=http(RCS,"POST","/api/job-lock",json!({"job_id":job.id,"requested_vehicle":job.requested_vehicle,"from":job.from}))?;
                job.vehicle_id=v["vehicle_id"].as_str().map(str::to_owned);change(job,"TO_SOURCE","RCS 已鎖定同一台車供整張作業使用");
            }
            "TO_SOURCE"|"TO_DEST" => {
                let source=job.step=="TO_SOURCE";
                let request_id=format!("{}-{}",job.id,if source{"source"}else{"dest"});
                let to=if source {job.from.clone()}else{job.to.clone()};
                // A car already at the requested station needs no artificial move.
                let fleet=http(RCS,"GET","/api/vehicles",Value::Null)?;
                let car=fleet["vehicles"].as_array().and_then(|vs|vs.iter().find(|v|v["vehicle_id"].as_str()==job.vehicle_id.as_deref())).ok_or("等待用車資訊")?;
                let existing=http(RCS,"GET","/api/state",Value::Null)?;
                let task=existing["tasks"].as_array().and_then(|ts|ts.iter().find(|t|t["request_id"]==request_id)).cloned();
                if let Some(task)=task {
                    if !job.task_ids.contains(&task["id"].as_str().unwrap_or("").to_string()) {job.task_ids.push(task["id"].as_str().unwrap().into());}
                    match task["status"].as_str().unwrap_or("") {
                        "COMPLETED"=>change(job,if source{"PICK"}else{"DROP"},"RCS 已確認移動完成，接著等待設備交接"),
                        "CANCELED"|"FAILED"|"INTERRUPTED"=>interrupt(job,"子任務中止或結果不確定，保留作業用車待人工處理"),
                        _=>wait(job,format!("等待 RCS 任務 {}：{}",task["id"].as_str().unwrap_or(""),task["wait_reason"].as_str().or(task["phase"].as_str()).unwrap_or("執行中"))),
                    }
                } else if car["online"]==true && car["active"].is_null() && car["current_node"]==to && car["pending_callbacks"]==0 {
                    change(job,if source{"PICK"}else{"DROP"},"車已在交接點，不建立原地移動任務");
                } else {
                    let from=car["current_node"].as_str().ok_or("車停在路段中，等待位置確認")?;
                    let v=http(RCS,"POST","/api/tasks",json!({"request_id":request_id,"kind":"MOVE","from":from,"to":to,"requested_vehicle":job.vehicle_id,"job_id":job.id}))?;
                    job.task_ids.push(v["task"]["id"].as_str().ok_or("缺少 RCS 任務編號")?.into());
                    wait(job,"RCS 已建立點到點子任務".into());
                }
            }
            "PICK"|"DROP" => {
                let pick=job.step=="PICK";
                let v=http(ECS,"POST","/api/interactions",json!({"command_id":format!("{}-{}",job.id,job.step),"job_id":job.id,"vehicle_id":job.vehicle_id,"station":if pick{&job.from}else{&job.to},"action":if pick{"PICK"}else{"DROP"}}))?;
                let op=&v["interaction"];
                match op["status"].as_str().unwrap_or("") {
                    "COMPLETED"=>{job.loaded=pick;change(job,if pick{"WAIT_DEST"}else{"RELEASE"},if pick{"ECS 已確認模擬取料完成"}else{"ECS 已確認模擬放料完成"});}
                    "INTERRUPTED"|"TIMED_OUT"|"FAILED"|"CANCELED"=>{let issue=serde_json::from_value(op["alarm"].clone()).unwrap_or_else(|_|limits::alarm("ECS_RESULT_UNCERTAIN","ECS",if pick{&job.from}else{&job.to},Some(format!("{}-{}",job.id,job.step)),"設備交接停止或結果待確認"));limits::raise(job,issue);},
                    _=>wait(job,format!("等待 ECS：{}",op["phase"].as_str().unwrap_or("設備交接"))),
                }
            }
            "WAIT_DEST" => {
                if device_ready(job,&job.to,false)? {change(job,"TO_DEST","目的設備可收料且無故障");}
                else {wait(job,format!("等待 {} 可收料／解除故障；用車保持鎖定",job.to));}
            }
            "RELEASE" => {
                http(RCS,"POST","/api/job-unlock",json!({"job_id":job.id}))?;
                change(job,"DONE","移動與模擬設備交接均完成，已解除作業用車鎖定");
            }
            _=>interrupt(job,"未知流程步驟"),
        }
        Ok(())
    })();
    if let Err(reason)=result {wait(job,reason);}
}

pub fn handler(role: &str, r: &Request) -> Response {
    if r.method=="GET" && r.path=="/" {return Response{code:200,body:include_str!("../web/services.html")
        .replace("__SERVICE__",role)
        .replace("<!-- APP-THEME -->",APP_THEME)
        .replace("<!-- APP-SHELL -->",&app_shell(role))
        .replace("<!-- SERVICES-THEME -->",include_str!("../web/services.css"))
        .replace("<!-- SERVICES-SCRIPT -->",include_str!("../web/services.js"))
        .replace("<!-- ATTENTION-SCRIPT -->",include_str!("../web/attention.js"))
        .into_bytes(),content_type:"text/html; charset=utf-8"};}
    if r.method=="GET" && r.path=="/health" {return ok(json!({"ok":true,"role":role,"mode":"simulation"}));}
    if r.method=="GET" && r.path=="/api/traces" {return ok(json!({"traces":trace_store().lock().unwrap().iter().cloned().collect::<Vec<_>>()}));}
    if r.method=="GET" && r.path=="/api/catalog" {return ok(catalog::catalog());}
    if role=="wcs" { jobs_handler(r) }else{ equipment_handler(r) }
}
fn jobs_handler(r: &Request) -> Response {
    let shared=JOBS.get().unwrap();
    if r.method=="GET"&&r.path=="/api/attention"{return ok(attention::snapshot(shared));}
    if r.method=="POST"&&r.path=="/api/attention/policy"{return attention::policy(shared,r);}
    if matches!(r.method.as_str(),"GET"|"POST")&&r.path=="/api/policy"{return limits::wcs_policy(shared,r);}
    if r.method=="GET" && r.path=="/api/dashboard" {
        let jobs={let s=shared.lock().unwrap();json!({"jobs":s.jobs,"policy":s.policy,"storage_error":s.storage_error})};
        let equipment=http(ECS,"GET","/api/equipment",Value::Null);
        let mut traces:Vec<Value>=trace_store().lock().unwrap().iter().map(|t|json!(t)).collect();
        if let Ok(v)=http(ECS,"GET","/api/traces",Value::Null){if let Some(items)=v["traces"].as_array(){traces.extend(items.iter().cloned());}}
        return ok(json!({"jobs":jobs["jobs"],"policy":jobs["policy"],"storage_error":jobs["storage_error"],"equipment":equipment.as_ref().ok(),"equipment_error":equipment.as_ref().err(),"traffic":traces,"attention":attention::snapshot(shared)}));
    }
    if r.method=="GET" && matches!(r.path.as_str(),"/api/jobs"|"/api/state") {
        let s=shared.lock().unwrap();return ok(json!({"role":"wcs","jobs":s.jobs,"policy":s.policy,"storage_error":s.storage_error}));
    }
    if r.method=="POST" && r.path=="/api/jobs" {
        #[derive(Deserialize)] struct NewJob {request_id:String,from:String,to:String,#[serde(default)]requested_vehicle:Option<String>}
        let n:NewJob=match parse(r){Ok(v)=>v,Err(e)=>return e};
        if n.request_id.is_empty()||n.request_id.len()>100||n.from==n.to||!equipment_station(&n.from)||!equipment_station(&n.to)
            ||n.requested_vehicle.as_deref().is_some_and(|id|vehicle_port(id).is_none()) {return error(400,"無效作業請求");}
        let mut s=shared.lock().unwrap();
        if s.storage_error.is_some(){return error(503,"WCS 儲存異常，無法確認作業已持久保存");}
        if let Some(job)=s.jobs.iter().find(|j|j.request_id==n.request_id) {
            return if job.from==n.from&&job.to==n.to&&job.requested_vehicle==n.requested_vehicle {ok(json!({"job":job,"duplicate":true}))}else{error(409,"相同請求編號已用於不同作業")};
        }
        if s.storage_error.is_some(){return error(503,"WCS 儲存異常");}
        if s.jobs.len()>=10000{return error(409,"作業數量達上限");}
        let mut job=Job{id:format!("JOB-{}-{}",now(),s.jobs.len()+1),request_id:n.request_id,from:n.from,to:n.to,requested_vehicle:n.requested_vehicle,vehicle_id:None,status:"QUEUED".into(),step:"WAIT_SOURCE".into(),paused:false,loaded:false,created_at:now(),updated_at:now(),wait_reason:None,task_ids:vec![],events:vec![json!({"at":now(),"step":"WAIT_SOURCE","message":"介接服務已建立整張作業，等待來源設備"})],cancel_requested_at:None,cancel_deadline_at:None,canceled_at:None,cancel_reason:None,cancel_phase:None,cancel_requires_review:false,cancel_evidence:Value::Null,cancel_verify_since:None,recovery:None,continuations:vec![],timeout_policy:s.policy.clone(),wait_started_at:None,wait_deadline_at:None,alarm:None,attention_since:None};
        limits::enter(&mut job,"WAIT_SOURCE");
        s.jobs.push(job.clone());save_jobs(&mut s);
        if s.storage_error.is_some(){return error(503,"作業保存失敗，請用原 request_id 重試");}
        let mut response=ok(json!({"job":job,"duplicate":false}));response.code=201;return response;
    }
    if r.method=="POST" && r.path.starts_with("/api/jobs/") && r.path.ends_with("/continue") {return continuation::request(shared,r);}
    if r.method=="POST" && r.path.starts_with("/api/jobs/") && r.path.ends_with("/recover") {
        return recovery::request(shared,r);
    }
    if r.method=="POST" && r.path.starts_with("/api/jobs/") && r.path.ends_with("/cancel") {
        return cancel::request(shared,r);
    }
    if r.method=="POST" && r.path.starts_with("/api/jobs/") && r.path.ends_with("/pause") {
        let id=r.path.trim_start_matches("/api/jobs/").trim_end_matches("/pause");
        let v:Value=match parse(r){Ok(v)=>v,Err(e)=>return e};
        let Some(paused)=v["paused"].as_bool()else{return error(400,"paused 需為布林值");};
        let mut s=shared.lock().unwrap();
        if s.storage_error.is_some(){return error(503,"WCS 儲存異常");}
        let Some(job)=s.jobs.iter_mut().find(|j|j.id==id)else{return error(404,"找不到作業");};
        if matches!(job.status.as_str(),"COMPLETED"|"CANCELED"|"CANCELING"|"RECOVERING"|"RESUMING"|"INTERRUPTED"){return error(409,"已結束、取消中或待確認作業不能恢復執行");}
        job.paused=paused;job.updated_at=now();save_jobs(&mut s);
        if s.storage_error.is_some(){return error(503,"設定保存失敗");}
        return ok(json!({"paused":paused}));
    }
    error(404,"未知 WCS API")
}
fn equipment_handler(r: &Request) -> Response {
    let mut s=EQUIPMENT.get().unwrap().lock().unwrap();
    if r.method=="GET" && matches!(r.path.as_str(),"/api/equipment"|"/api/state") {return ok(json!({"role":"ecs","policy":s.policy,"devices":s.devices,"interactions":s.interactions,"reconciliations":s.reconciliations,"confirmed_results":s.confirmed_results,"canceled_jobs":s.canceled_jobs,"storage_error":s.storage_error}));}
    if s.storage_error.is_some(){return error(503,"ECS 儲存異常");}
    if matches!(r.method.as_str(),"GET"|"POST")&&r.path=="/api/policy"{return limits::ecs_policy(&mut s,r);}
    if r.method=="POST" && r.path=="/api/interactions/confirm-result" {return continuation::equipment(&mut s,r);}
    if r.method=="POST" && r.path=="/api/interactions/reconcile-job" {
        return recovery::equipment(&mut s,r);
    }
    if r.method=="POST" && r.path=="/api/interactions/cancel-job" {
        return cancel::equipment(&mut s,r);
    }
    if r.method=="POST" && r.path=="/api/equipment/sim/signals" {
        #[derive(Deserialize)]struct Signals{id:String,material_ready:bool,can_receive:bool,fault:bool,auto_complete:bool}
        let v:Signals=match parse(r){Ok(v)=>v,Err(e)=>return e};
        let Some(d)=s.devices.iter_mut().find(|d|d.id==v.id)else{return error(404,"找不到模擬設備");};
        *d=Device{id:v.id,material_ready:v.material_ready,can_receive:v.can_receive,fault:v.fault,auto_complete:v.auto_complete};
        save_equipment(&mut s);
        return if s.storage_error.is_none(){ok(json!({"ok":true,"simulation":true}))}else{error(503,"信號保存失敗")};
    }
    if r.method=="POST" && r.path=="/api/interactions" {
        #[derive(Deserialize)]struct Command{command_id:String,job_id:String,vehicle_id:String,station:String,action:String}
        let v:Command=match parse(r){Ok(v)=>v,Err(e)=>return e};
        if s.canceled_jobs.contains(&v.job_id){return error(409,"整張作業已要求取消，拒絕新增設備交接");}
        if !matches!(v.action.as_str(),"PICK"|"DROP")||v.command_id.is_empty()||v.command_id.len()>180||v.job_id.is_empty()||vehicle_port(&v.vehicle_id).is_none()||!equipment_station(&v.station){return error(400,"無效設備操作");}
        if let Some(op)=s.interactions.iter().find(|op|op.command_id==v.command_id) {
            return if op.job_id==v.job_id&&op.station==v.station&&op.action==v.action&&op.vehicle_id==v.vehicle_id{ok(json!({"interaction":op,"duplicate":true}))}else{error(409,"command_id 已用於不同設備操作")};
        }
        if s.interactions.iter().any(|op|op.station==v.station&&matches!(op.status.as_str(),"REQUESTED"|"ALLOWED"|"INTERRUPTED"|"TIMED_OUT"|"FAILED")&&!s.reconciliations.iter().any(|r|r.job_id==op.job_id)&&!s.confirmed_results.iter().any(|r|r.command_id==op.command_id)){return error(409,"設備交接中或有待確認交接");}
        if s.interactions.len()>=20000{return error(409,"設備操作紀錄達上限");}
        let op=Interaction{command_id:v.command_id,job_id:v.job_id,vehicle_id:v.vehicle_id,station:v.station,action:v.action,status:"REQUESTED".into(),requested_at:now(),allowed_at:None,completed_at:None,phase:"已通知設備，等待允許信號（模擬）".into(),canceled_at:None,timeout_policy:s.policy.clone(),allow_deadline_at:Some(now().saturating_add(s.policy.allow_wait_ms)),completion_deadline_at:None,alarm:None};
        s.interactions.push(op.clone());save_equipment(&mut s);
        return if s.storage_error.is_none(){ok(json!({"interaction":op,"duplicate":false}))}else{error(503,"設備操作保存失敗")};
    }
    error(404,"未知 ECS API")
}
