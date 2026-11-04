//! Station admission for the simulator. This does not control shared road segments.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct Lease {
    pub node: String,
    pub vehicle_id: String,
    pub task_id: String,
    pub stage: String,
    pub at: u64,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Traffic {
    #[serde(default)]
    pub leases: Vec<Lease>,
    // Retain last known positions across disconnects and RCS restarts.
    #[serde(default)]
    pub observations: Vec<Value>,
}
pub fn location(v: &Value) -> Option<&'static str> {
    let (x, y) = (v["x"].as_f64()?, v["y"].as_f64()?);
    NODES
        .iter()
        .find(|n| (x - n.1).abs() < 0.001 && (y - n.2).abs() < 0.001)
        .map(|n| n.0)
}
impl Traffic {
    pub fn refresh(&mut self, fleet: &[Value]) {
        for v in fleet {
            let id = v["vehicle_id"].as_str().unwrap_or("");
            let old = self.observations.iter().position(|o| o["vehicle_id"] == id);
            if v["online"] == true {
                let mut value = v.clone();
                value["observed_at"] = v["sample_at"]
                    .as_u64()
                    .map_or_else(|| json!(now()), |at| json!(at));
                if old.is_some_and(|i| {
                    self.observations[i]["observed_at"].as_u64().unwrap_or(0)
                        > value["observed_at"].as_u64().unwrap_or(0)
                }) {
                    continue;
                }
                if let Some(i) = old {
                    self.observations[i] = value;
                } else {
                    self.observations.push(value);
                }
            } else if let Some(i) = old {
                self.observations[i]["online"] = json!(false);
            } else {
                self.observations.push(v.clone());
            }
        }
    }
    pub fn blockers(&self, target: &str, own: &str) -> Vec<String> {
        let mut out = vec![];
        for v in &self.observations {
            let id = v["vehicle_id"].as_str().unwrap_or("unknown");
            if id == own {
                continue;
            }
            if v["online"] == true
                && now().saturating_sub(v["observed_at"].as_u64().unwrap_or(0)) > 5000
            {
                out.push(format!("{id} 位置更新逾時，暫不允許進站"));
            }
            if location(v) == Some(target) {
                out.push(format!(
                    "{id} 占用 {target}{}",
                    if v["online"] == true {
                        ""
                    } else {
                        "（離線，保留最後位置）"
                    }
                ));
            } else if v["online"] != true && v["x"].is_null() {
                out.push(format!("{id} 位置未知，暫不允許進站"));
            }
        }
        for l in &self.leases {
            if l.node == target && l.vehicle_id != own {
                out.push(format!(
                    "{} 已預留 {}（{}）",
                    l.vehicle_id, target, l.task_id
                ));
            }
        }
        out
    }
    pub fn view(&self) -> Value {
        json!({"stations":NODES.iter().map(|n| {
            let occupants: Vec<Value> = self.observations.iter().filter(|v| location(v)==Some(n.0)).map(|v| json!({"vehicle_id":v["vehicle_id"],"online":v["online"],"observed_at":v["observed_at"]})).collect();
            let reservations: Vec<&Lease> = self.leases.iter().filter(|l| l.node==n.0).collect();
            json!({"node":n.0,"occupants":occupants,"reservations":reservations})
        }).collect::<Vec<_>>(),"observations":self.observations,"scope":"station admission only; no road traffic control"})
    }
}

#[derive(Deserialize)]
struct Entry {
    task_id: String,
    vehicle_id: String,
    stage: String,
}
pub fn entry(s: &Shared<RcsState>, r: &Request) -> Response {
    let e: Entry = match parse(r) {
        Ok(e) => e,
        Err(e) => return e,
    };
    let mut s = s.lock().unwrap();
    if s.storage_error.is_some() {
        return error(503, "儲存異常，暫停進站許可");
    }
    let Some(i) = s.tasks.iter().position(|t| t.id == e.task_id) else {
        return error(404, "找不到任務");
    };
    let t = &s.tasks[i];
    if t.job_id.as_deref().is_some_and(|job|workflow::canceled(&s,job)) {
        return error(409,"整張作業正在取消，停止核准新的路徑");
    }
    if t.assigned_vehicle.as_deref() != Some(&e.vehicle_id)
        || !matches!(t.status.as_str(), "RUNNING" | "DISPATCHING")
    {
        return error(409, "車號或任務狀態不符合進站條件");
    }
    let target = match e.stage.as_str() {
        "pickup" if t.kind != "YIELD" => t.from.clone(),
        "destination" if t.kind != "YIELD" => t.to.clone(),
        "move" if t.kind == "YIELD" => t.to.clone(),
        _ => {
            return error(
                400,
                "stage 與任務類型不符；搬運用 pickup/destination，讓位用 move",
            )
        }
    };
    // Telemetry is refreshed outside the RCS mutex. Grants are serialized here.
    // Movement is fenced by persistent leases, including terminal tasks awaiting
    // a fresh final position. Old telemetry can delay entry, never release a lease.
    let Some(car) = s
        .traffic
        .observations
        .iter()
        .find(|v| v["vehicle_id"] == e.vehicle_id && v["online"] == true)
        .cloned()
    else {
        return error(409, "等待申請車輛連線資訊");
    };
    if s.traffic.observations.len() != VEHICLES.len()
        || now().saturating_sub(car["observed_at"].as_u64().unwrap_or(0)) > 5000
    {
        return ok(json!({"granted":false,"node":target,"reason":"等待更新車隊位置"}));
    }
    if car["active"]["task_id"] != e.task_id {
        return error(409, "車端執行的任務不符");
    }
    let current = s
        .traffic
        .leases
        .iter()
        .find(|l| l.task_id == e.task_id)
        .cloned();
    let valid = if e.stage == "move" {
        current
            .as_ref()
            .is_some_and(|l| l.stage == "move" && l.node == target)
            && location(&car) == Some(s.tasks[i].from.as_str())
            && car["progress"] == 0
    } else if e.stage == "pickup" {
        current.as_ref().is_none_or(|l| l.stage == "pickup") && car["progress"].as_u64() == Some(0)
    } else {
        current
            .as_ref()
            .is_some_and(|l| l.stage == "pickup" || l.stage == "destination")
            && location(&car) == Some(s.tasks[i].from.as_str())
            && car["progress"].as_u64() == Some(37)
    };
    if !valid {
        return error(409, "尚未抵達正確的進站申請階段");
    }
    let mut blockers = s.traffic.blockers(&target, &e.vehicle_id);
    for child in s.tasks.iter().filter(|t| {
        t.kind == "YIELD"
            && t.parent_task.as_deref() == Some(&e.task_id)
            && t.from == target
            && t.status != "COMPLETED"
    }) {
        blockers.push(format!(
            "等待讓位任務 {} 到達 {}（{}）",
            child.id, child.to, child.status
        ));
    }
    if !blockers.is_empty() {
        let plan = yielding::plan(&mut s, i, &target, &e.vehicle_id);
        let reason = format!("等待進入 {target}：{}；{plan}", blockers.join("；"));
        if s.tasks[i].wait_reason.as_deref() != Some(&reason) {
            s.tasks[i].wait_since.get_or_insert(now());
            s.tasks[i].wait_reason = Some(reason.clone());
            s.tasks[i].updated_at = now();
            log_event(&mut s, &e.task_id, &reason);
            save(&mut s);
        }
        return ok(json!({"granted":false,"node":target,"reason":reason,"blockers":blockers}));
    }
    let grant = match navigation::reserve(&mut s, i, &e.stage, &target, &car) {
        Ok(g) => g,
        Err(reason) => {
            if s.tasks[i].wait_reason.as_deref() != Some(&reason) {
                s.tasks[i].wait_reason = Some(reason.clone());
                s.tasks[i].wait_since.get_or_insert(now());
                log_event(&mut s, &e.task_id, &reason);
                save(&mut s);
            }
            return ok(json!({"granted":false,"node":target,"reason":reason}));
        }
    };
    let already = current.as_ref().is_some_and(|l| l.stage == e.stage);
    if !already {
        s.traffic.leases.retain(|l| l.task_id != e.task_id);
        s.traffic.leases.push(Lease {
            node: target.clone(),
            vehicle_id: e.vehicle_id.clone(),
            task_id: e.task_id.clone(),
            stage: e.stage,
            at: now(),
        });
        log_event(
            &mut s,
            &e.task_id,
            &format!("進站許可：{} 預留 {target}", e.vehicle_id),
        );
    }
    if s.tasks[i].wait_reason.is_some() {
        s.tasks[i].updated_at = now();
    }
    s.tasks[i].wait_reason = None;
    s.tasks[i].wait_since = None;
    save(&mut s);
    if s.storage_error.is_some() {
        return error(503, "進站許可儲存失敗，車輛繼續等待");
    }
    ok(
        json!({"granted":true,"node":target,"vehicle_id":e.vehicle_id,"task_id":e.task_id,"path":grant.path,"route_token":grant.token}),
    )
}

pub fn park(s: &Shared<RcsState>, r: &Request) -> Response {
    let p: Value = match parse(r) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let (Some(id), Some(target)) = (p["vehicle_id"].as_str(), p["node"].as_str()) else {
        return error(400, "請指定 vehicle_id 和 node");
    };
    let Some(port) = vehicle_port(id) else {
        return error(400, "未知車號");
    };
    if node(target).is_none() {
        return error(400, "未知點位");
    }
    let mut s = s.lock().unwrap();
    if s.storage_error.is_some() {
        return error(503, "儲存異常");
    }
    let fleet = fleet_states();
    s.traffic.refresh(&fleet);
    let Some(car) = fleet
        .iter()
        .find(|v| v["vehicle_id"] == id && v["online"] == true)
    else {
        return error(409, "車輛未連線");
    };
    if s.job_locks.iter().any(|l|l.vehicle_id==id)
        || !car["active"].is_null()
        || car["pending_callbacks"] != 0
        || s.tasks
            .iter()
            .any(|t| !terminal(&t.status) && t.assigned_vehicle.as_deref() == Some(id))
        || s.roads.leases.iter().any(|l| l.vehicle_id == id)
        || s.traffic.leases.iter().any(|l| l.vehicle_id == id)
    {
        return error(409, "只能設定沒有任務、沒有預留且回報已送完的模擬車");
    }
    let mut blockers = s.traffic.blockers(target, id);
    blockers.extend(s.roads.conflict(&[target.into()], id));
    if !blockers.is_empty() {
        return error(409, &blockers.join("；"));
    }
    match http(port, "POST", "/api/sim/position", json!({"node":target})) {
        Ok(v) => {
            s.traffic.refresh(&fleet_states());
            log_event(
                &mut s,
                "SYSTEM",
                &format!("模擬設定位置：{id} → {target}（直接定位，非行駛任務）"),
            );
            save(&mut s);
            ok(v)
        }
        Err(e) => error(502, &e),
    }
}

pub fn demo(s: &Shared<RcsState>) -> Response {
    let mut s = s.lock().unwrap();
    if s.storage_error.is_some() {
        return error(503, "儲存異常");
    }
    if !s.job_locks.is_empty() || s.tasks.iter().any(|t| !terminal(&t.status))
        || !s.traffic.leases.is_empty()
        || !s.roads.leases.is_empty()
    {
        return error(409, "請先完成或取消現有任務；待確認的預留也須先處理");
    }
    let fleet = fleet_states();
    if fleet
        .iter()
        .any(|v| v["online"] != true || !v["active"].is_null() || v["pending_callbacks"] != 0)
    {
        return error(409, "情境設定需要三台車連線、待命且回報已送完");
    }
    // Explicit simulator-only scene setup. No real vehicle motion is requested.
    for (id, target) in [("CT-SIM-01", "P1"), ("CT-SIM-02", "B"), ("CT-SIM-03", "P3")] {
        if let Err(e) = http(
            vehicle_port(id).unwrap(),
            "POST",
            "/api/sim/position",
            json!({"node":target}),
        ) {
            return error(502, &format!("情境位置設定未完成：{e}"));
        }
    }
    s.traffic.refresh(&fleet_states());
    let ts = now();
    let t = Task {
        job_id: None,
        kind: "TRANSPORT".into(),
        parent_task: None,
        id: format!("CT-{ts}-{}", s.tasks.len() + 1),
        request_id: format!("station-demo-{ts}"),
        from: "A".into(),
        to: "B".into(),
        requested_vehicle: Some("CT-SIM-01".into()),
        assigned_vehicle: None,
        status: "QUEUED".into(),
        phase: "等待派車".into(),
        progress: 0,
        created_at: ts,
        updated_at: ts,
        note: "點位占用測試：AGV02 停在 B；手動移至 P2 後 AGV01 才能進 B。".into(),
        last_seq: 0,
        timing: TaskTiming::default(),
        routes: vec![],
        wait_reason: None,
        wait_since: None,
    };
    s.tasks.push(t.clone());
    s.auto_dispatch = true;
    log_event(
        &mut s,
        &t.id,
        "建立 B 點占用情境：指定 AGV01 執行 A → B，AGV02 占用 B",
    );
    save(&mut s);
    if s.storage_error.is_some() {
        return error(503, "情境任務儲存失敗");
    }
    response(
        201,
        json!({"task":t,"message":"情境已建立；AGV01 在 A 取貨後等待 B 清空"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_or_missing_telemetry_cannot_make_a_station_look_free() {
        let mut t = Traffic::default();
        let ts = now();
        t.refresh(&[json!({"vehicle_id":"CT-SIM-02","online":true,"x":80,"y":25,"sample_at":ts})]);
        t.refresh(&[
            json!({"vehicle_id":"CT-SIM-02","online":true,"x":48,"y":64,"sample_at":ts-1}),
        ]);
        assert_eq!(location(&t.observations[0]), Some("B"));
        assert!(!t.blockers("B", "CT-SIM-01").is_empty());
        let mut stale = Traffic::default();
        stale.refresh(&[
            json!({"vehicle_id":"CT-SIM-02","online":true,"x":48,"y":64,"sample_at":ts-6000}),
        ]);
        assert!(!stale.blockers("B", "CT-SIM-01").is_empty());
        let mut unknown = Traffic::default();
        unknown.refresh(&[json!({"vehicle_id":"CT-SIM-03","online":false})]);
        assert!(!unknown.blockers("A", "CT-SIM-01").is_empty());
    }
    #[test]
    fn occupancy_reservation_and_offline_position_block_entry() {
        let mut t = Traffic::default();
        t.refresh(&[json!({"vehicle_id":"CT-SIM-02","online":true,"x":80,"y":25})]);
        assert!(!t.blockers("B", "CT-SIM-01").is_empty());
        assert!(t.blockers("B", "CT-SIM-02").is_empty());
        t.refresh(&[json!({"vehicle_id":"CT-SIM-02","online":false})]);
        assert!(!t.blockers("B", "CT-SIM-01").is_empty());
        t.refresh(&[json!({"vehicle_id":"CT-SIM-02","online":true,"x":48,"y":64})]);
        assert!(t.blockers("B", "CT-SIM-01").is_empty());
        t.leases.push(Lease {
            node: "B".into(),
            vehicle_id: "CT-SIM-03".into(),
            task_id: "T".into(),
            stage: "destination".into(),
            at: now(),
        });
        assert!(!t.blockers("B", "CT-SIM-01").is_empty());
        let restored: Traffic = serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
        assert!(!restored.blockers("B", "CT-SIM-01").is_empty());
    }
}
