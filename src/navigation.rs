//! Directed road graph and conservative, atomic corridor reservations.
use super::*;
pub const JUNCTIONS: [(&str, f64, f64); 18] = [
    ("J1", 15., 52.),
    ("J2", 48., 25.),
    ("J3", 80., 52.),
    ("J4", 48., 78.),
    ("J5", 48., 42.),
    ("J6", 30., 42.),
    ("J7", 65., 42.),
    ("J8", 15., 16.),
    ("J9", 36., 16.),
    ("J10", 58., 16.),
    ("J11", 80., 16.),
    ("J12", 90., 25.),
    ("J13", 90., 52.),
    ("J14", 90., 78.),
    ("J15", 15., 85.),
    ("J16", 36., 85.),
    ("J17", 58., 85.),
    ("J18", 80., 85.),
];
pub const EDGES: [(&str, &str, bool); 41] = [
    ("A", "J2", true),
    ("J2", "B", true),
    ("B", "J3", true),
    ("J3", "WIP", true),
    ("WIP", "J4", true),
    ("J4", "C", true),
    ("C", "J1", true),
    ("J1", "A", true),
    ("J2", "J5", false),
    ("J5", "J6", true),
    ("J6", "J1", true),
    ("J5", "J7", true),
    ("J7", "J3", true),
    ("J6", "P1", true),
    ("J7", "P3", true),
    ("J5", "HOME", true),
    ("J4", "P2", true),
    ("A", "J8", true),
    ("J8", "J9", true),
    ("J9", "J10", true),
    ("J10", "J11", true),
    ("J11", "B", true),
    ("J8", "CUT", true),
    ("J9", "CNC", true),
    ("J10", "OVEN", true),
    ("J11", "PACK", true),
    ("B", "J12", true),
    ("J12", "J13", true),
    ("J13", "J14", true),
    ("J14", "WIP", true),
    ("J12", "CHG1", true),
    ("J14", "CHG2", true),
    ("C", "J15", true),
    ("J15", "J16", true),
    ("J16", "J17", true),
    ("J17", "J18", true),
    ("J18", "WIP", true),
    ("J15", "S1", true),
    ("J16", "S2", true),
    ("J17", "I1", true),
    ("J18", "I2", true),
];
pub fn xy(id: &str) -> Option<(f64, f64)> {
    node(id).or_else(|| JUNCTIONS.iter().find(|n| n.0 == id).map(|n| (n.1, n.2)))
}
pub fn at(x: f64, y: f64) -> Option<&'static str> {
    NODES
        .iter()
        .chain(JUNCTIONS.iter())
        .find(|n| (x - n.1).abs() < 0.001 && (y - n.2).abs() < 0.001)
        .map(|n| n.0)
}
pub fn legal(a: &str, b: &str) -> bool {
    EDGES
        .iter()
        .any(|e| (e.0 == a && e.1 == b) || (e.2 && e.1 == a && e.0 == b))
}
pub fn edge_id(a: &str, b: &str) -> String {
    if a < b {
        format!("{a}--{b}")
    } else {
        format!("{b}--{a}")
    }
}
pub fn length(a: &str, b: &str) -> f64 {
    let a = xy(a).unwrap();
    let b = xy(b).unwrap();
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}
pub fn shortest(start: &str, end: &str, blocked: &[String]) -> Option<Vec<String>> {
    let nodes: Vec<&str> = NODES.iter().chain(JUNCTIONS.iter()).map(|n| n.0).collect();
    let (Some(si), Some(ei)) = (
        nodes.iter().position(|n| *n == start),
        nodes.iter().position(|n| *n == end),
    ) else {
        return None;
    };
    let mut d = vec![f64::INFINITY; nodes.len()];
    let mut prev = vec![None; nodes.len()];
    let mut used = vec![false; nodes.len()];
    d[si] = 0.;
    for _ in 0..nodes.len() {
        let u = (0..nodes.len())
            .filter(|i| !used[*i])
            .min_by(|a, b| d[*a].total_cmp(&d[*b]))?;
        if !d[u].is_finite() {
            break;
        }
        if u == ei {
            break;
        }
        used[u] = true;
        for v in 0..nodes.len() {
            if !used[v]
                && legal(nodes[u], nodes[v])
                && (v == ei || v == si || !blocked.iter().any(|n| n == nodes[v]))
            {
                let cost = d[u] + length(nodes[u], nodes[v]);
                if cost < d[v] {
                    d[v] = cost;
                    prev[v] = Some(u);
                }
            }
        }
    }
    if !d[ei].is_finite() {
        return None;
    }
    let mut path = vec![nodes[ei].to_string()];
    let mut i = ei;
    while i != si {
        i = prev[i]?;
        path.push(nodes[i].into());
    }
    path.reverse();
    Some(path)
}
pub fn station_meta(id: &str) -> (&'static str, &'static str) {
    match id {
        "A" => ("組裝機 A", "machine"),
        "B" => ("檢測機 B", "machine"),
        "C" => ("清洗機 C", "machine"),
        "WIP" => ("立庫出入庫", "warehouse"),
        "HOME" => ("待命總站", "standby"),
        "P1" => ("避讓點 P1", "passing"),
        "P2" => ("避讓點 P2", "passing"),
        "P3" => ("避讓點 P3", "passing"),
        "CUT" => ("雷射切割機", "machine"),
        "CNC" => ("CNC 加工機", "machine"),
        "OVEN" => ("烘烤爐", "machine"),
        "PACK" => ("包裝機", "machine"),
        "CHG1" => ("充電站 01", "charging"),
        "CHG2" => ("充電站 02", "charging"),
        "S1" => ("待命席 01", "standby"),
        "S2" => ("待命席 02", "standby"),
        "I1" => ("空閒停車 01", "idle"),
        "I2" => ("空閒停車 02", "idle"),
        _ => ("未知點位", "station"),
    }
}
pub fn graph() -> Value {
    json!({"nodes":NODES.iter().map(|n|json!({"id":n.0,"x":n.1,"y":n.2,"kind":"station","label":station_meta(n.0).0,"category":station_meta(n.0).1})).chain(JUNCTIONS.iter().map(|n|json!({"id":n.0,"x":n.1,"y":n.2,"kind":"junction"}))).collect::<Vec<_>>(),"edges":EDGES.iter().map(|e|json!({"id":edge_id(e.0,e.1),"from":e.0,"to":e.1,"bidirectional":e.2,"length":length(e.0,e.1)})).collect::<Vec<_>>(),"units":"schematic coordinates","policy":"reserve whole leg atomically; release after confirmed arrival"})
}
#[derive(Clone, Serialize, Deserialize)]
pub struct RouteGrant {
    pub token: String,
    pub task_id: String,
    pub vehicle_id: String,
    pub stage: String,
    pub path: Vec<String>,
    pub granted_at: u64,
    #[serde(default)]
    pub released_at: Option<u64>,
}
impl RouteGrant {
    pub fn edges(&self) -> Vec<String> {
        self.path
            .windows(2)
            .map(|p| edge_id(&p[0], &p[1]))
            .collect()
    }
}
#[derive(Default, Serialize, Deserialize)]
pub struct RoadState {
    #[serde(default)]
    pub leases: Vec<RouteGrant>,
}
impl RoadState {
    pub fn conflict(&self, path: &[String], own: &str) -> Vec<String> {
        self.leases
            .iter()
            .filter(|l| l.vehicle_id != own && l.path.iter().any(|n| path.contains(n)))
            .map(|l| {
                format!(
                    "{} 預約路徑 {}（{}）",
                    l.vehicle_id,
                    l.path.join(" → "),
                    l.task_id
                )
            })
            .collect()
    }
    pub fn view(&self) -> Value {
        json!({"graph":graph(),"reservations":self.leases,"reserved_edges":self.leases.iter().flat_map(|l|l.edges()).collect::<Vec<_>>()})
    }
}

pub fn recover(s: &Shared<RcsState>, r: &Request) -> Response {
    let p: Value = match parse(r) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let (Some(id), Some(target)) = (p["vehicle_id"].as_str(), p["node"].as_str()) else {
        return error(400, "請指定車號和等待點");
    };
    let Some(port) = vehicle_port(id) else {
        return error(400, "未知車號");
    };
    if !PARKING_NODES.contains(&target) {
        return error(400, "模擬復歸僅可選避讓點 P1/P2/P3 或空閒位 I1/I2");
    }
    let mut s = s.lock().unwrap();
    if s.storage_error.is_some() {
        return error(503, "儲存異常，不允許復歸");
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
            .any(|t| t.assigned_vehicle.as_deref() == Some(id) && !terminal(&t.status))
    {
        return error(409, "請先取消車端任務並等待回報送完，才能模擬復歸");
    }
    let mut blockers = s.traffic.blockers(target, id);
    blockers.extend(s.roads.conflict(&[target.into()], id));
    if !blockers.is_empty() {
        return error(409, &blockers.join("；"));
    }
    if let Err(e) = http(port, "POST", "/api/sim/position", json!({"node":target})) {
        return error(502, &format!("復歸位置尚未確認，保留預約：{e}"));
    }
    s.roads.leases.retain(|l| l.vehicle_id != id);
    let done: Vec<String> = s
        .tasks
        .iter()
        .filter(|t| terminal(&t.status) && t.assigned_vehicle.as_deref() == Some(id))
        .map(|t| t.id.clone())
        .collect();
    s.traffic.leases.retain(|l| !done.contains(&l.task_id));
    for t in s.tasks.iter_mut().filter(|t| done.contains(&t.id)) {
        for path in &mut t.routes {
            if path.released_at.is_none() {
                path.released_at = Some(now());
            }
        }
    }
    s.traffic.refresh(&fleet_states());
    log_event(
        &mut s,
        "SYSTEM",
        &format!("操作員模擬復歸：{id} 已設定至 {target}，解除該車待確認的預約（非真車移動）"),
    );
    save(&mut s);
    if s.storage_error.is_some() {
        return error(503, "復歸資料儲存失敗，暫停放行");
    }
    ok(
        json!({"ok":true,"vehicle_id":id,"node":target,"message":"模擬復歸已完成；這是直接設定模擬位置，非真車或行駛任務"}),
    )
}
pub fn release_arrived(s: &mut RcsState) {
    let mut released = vec![];
    s.roads.leases.retain(|l| {
        let allowed = s
            .tasks
            .iter()
            .find(|t| t.id == l.task_id)
            .is_some_and(|t| t.status != "INTERRUPTED");
        let arrived = allowed
            && s.traffic.observations.iter().any(|v| {
                v["online"] == true
                    && v["vehicle_id"] == l.vehicle_id
                    && v["route_token"] == l.token
                    && v["road_edge"].is_null()
                    && v["current_node"].as_str() == l.path.last().map(String::as_str)
                    && v["sample_at"].as_u64().unwrap_or(0) >= l.granted_at
            });
        if arrived {
            released.push((l.task_id.clone(), l.token.clone()));
        }
        !arrived
    });
    if !released.is_empty() {
        for (id, token) in released {
            if let Some(t) = s.tasks.iter_mut().find(|t| t.id == id) {
                if let Some(r) = t.routes.iter_mut().find(|r| r.token == token) {
                    r.released_at = Some(now());
                }
            }
            log_event(s, &id, "已確認到達路徑終點，釋放路段與路口預約");
        }
        save(s);
    }
}
pub fn reserve(
    s: &mut RcsState,
    i: usize,
    stage: &str,
    target: &str,
    car: &Value,
) -> Result<RouteGrant, String> {
    let id = s.tasks[i].id.clone();
    let own = s.tasks[i].assigned_vehicle.clone().unwrap();
    if let Some(l) = s.roads.leases.iter().find(|l| l.vehicle_id == own) {
        if l.task_id == id && l.stage == stage {
            return Ok(l.clone());
        }
        return Err("前一趟路徑的到站結果尚未確認".into());
    }
    let start = car["current_node"]
        .as_str()
        .ok_or("車輛停在路段中，需確認位置後才能重新規劃")?;
    let blocked: Vec<String> = s
        .traffic
        .observations
        .iter()
        .filter(|v| v["vehicle_id"] != own)
        .filter_map(|v| v["current_node"].as_str().or_else(|| traffic::location(v)))
        .map(str::to_owned)
        .collect();
    let path =
        shortest(start, target, &blocked).ok_or("沒有可通行路徑；沿線點位被占用或方向不允許")?;
    let conflicts = s.roads.conflict(&path, &own);
    if !conflicts.is_empty() {
        return Err(format!("等待路段／路口：{}", conflicts.join("；")));
    }
    let ts = now();
    let grant = RouteGrant {
        token: format!("{id}-{stage}-{ts}"),
        task_id: id,
        vehicle_id: own,
        stage: stage.into(),
        path,
        granted_at: ts,
        released_at: None,
    };
    s.roads.leases.push(grant.clone());
    s.tasks[i].routes.push(grant.clone());
    log_event(
        s,
        &grant.task_id,
        &format!("預約路徑：{}", grant.path.join(" → ")),
    );
    Ok(grant)
}
#[derive(Clone)]
pub struct Motion {
    pub path: Vec<String>,
    pub index: usize,
    pub offset: f64,
    pub traveled: f64,
    pub total: f64,
}
impl Motion {
    pub fn new(path: Vec<String>) -> Option<Self> {
        if path.is_empty() || path.windows(2).any(|p| !legal(&p[0], &p[1])) {
            return None;
        }
        let total = path.windows(2).map(|p| length(&p[0], &p[1])).sum();
        Some(Self {
            path,
            index: 0,
            offset: 0.,
            traveled: 0.,
            total,
        })
    }
    pub fn step(&mut self) -> ((f64, f64), Option<(String, String)>, bool) {
        if self.index + 1 >= self.path.len() {
            return (xy(self.path.last().unwrap()).unwrap(), None, true);
        }
        let a = &self.path[self.index];
        let b = &self.path[self.index + 1];
        let distance = length(a, b);
        let delta = 8_f64.min(distance - self.offset);
        self.offset += delta;
        self.traveled += delta;
        let start = xy(a).unwrap();
        let end = xy(b).unwrap();
        let f = self.offset / distance;
        let pos = (
            start.0 + (end.0 - start.0) * f,
            start.1 + (end.1 - start.1) * f,
        );
        if self.offset + 0.0001 >= distance {
            self.index += 1;
            self.offset = 0.;
            (pos, None, self.index + 1 >= self.path.len())
        } else {
            (pos, Some((a.clone(), b.clone())), false)
        }
    }
}
pub fn advance(v: &mut Vehicle) {
    let Some(mut a) = v.active.take() else { return };
    a.seq += 1;
    let moving = a.mission.kind == "YIELD";
    let point_to_point = a.mission.kind == "MOVE";
    let mut done = false;
    if let Some(mut motion) = a.motion.take() {
        let (pos, edge, arrived) = motion.step();
        v.x = pos.0;
        v.y = pos.1;
        v.road_edge = edge;
        let f = if motion.total > 0. {
            motion.traveled / motion.total
        } else {
            1.
        };
        v.progress = if moving {
            (f * 99.) as u32
        } else if point_to_point {
            if a.ticks == 0 { (f * 37.) as u32 } else { 37 + (f * 62.) as u32 }
        } else if a.ticks == 0 {
            (f * 25.) as u32
        } else {
            37 + (f * 50.) as u32
        };
        v.phase = if moving {
            "沿路網讓位移動"
        } else if point_to_point && a.ticks == 0 {
            "沿路網前往移動起點"
        } else if point_to_point {
            "沿路網點到點移動"
        } else if a.ticks == 0 {
            "沿路網前往取料點"
        } else {
            "沿路網搬運至目的地"
        }
        .into();
        if arrived {
            if moving {
                done = true;
                v.progress = 100;
                v.phase = "讓位完成".into();
            } else if point_to_point {
                if a.ticks == 0 {
                    a.ticks = 12;
                    v.progress = 37;
                    v.phase = "已抵達移動起點，等待下一段通行許可".into();
                } else {
                    done = true;
                    v.progress = 100;
                    v.phase = "點到點移動完成".into();
                }
            } else {
                a.ticks = if a.ticks == 0 { 8 } else { 28 };
                v.progress = if a.ticks == 8 { 25 } else { 87 };
                v.phase = "已抵達站點".into();
            }
        } else {
            a.motion = Some(motion);
        }
    } else {
        // Only the stationary loading/unloading phases advance without a route grant.
        if matches!(a.ticks, 0 | 12) {
            v.active = Some(a);
            return;
        }
        a.ticks += 1;
        v.progress = a.ticks.min(32) * 100 / 32;
        done = a.ticks >= 32;
        v.phase = if done {
            "搬運完成"
        } else if a.ticks <= 12 {
            "取料中（模擬）"
        } else {
            "放料中（模擬）"
        }
        .into();
    }
    let c = Callback {
        task_id: a.mission.task_id.clone(),
        vehicle_id: vehicle_id().into(),
        seq: a.seq,
        status: if done { "COMPLETED" } else { "RUNNING" }.into(),
        progress: v.progress,
        phase: v.phase.clone(),
    };
    v.last_task = Some(c.clone());
    v.pending.push_back(c);
    if !done {
        v.active = Some(a);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expanded_factory_all_stations_have_directed_round_trips() {
        assert_eq!(NODES.len(), 18);
        assert_eq!(JUNCTIONS.len(), 18);
        assert_eq!(EDGES.len(), 41);
        for (start, _, _) in NODES {
            for (end, _, _) in NODES {
                let route = shortest(start, end, &[])
                    .unwrap_or_else(|| panic!("No route from {start} to {end}"));
                assert!(route.windows(2).all(|p| legal(&p[0], &p[1])));
            }
        }
        assert_eq!(station_meta("CHG1").1, "charging");
        assert_eq!(station_meta("S1").1, "standby");
        assert_eq!(station_meta("I1").1, "idle");
    }
    #[test]
    fn dijkstra_respects_direction_and_occupied_nodes() {
        let p = shortest("A", "B", &[]).unwrap();
        assert_eq!(p, vec!["A", "J2", "B"]);
        assert!(!legal("J5", "J2"));
        let p = shortest("HOME", "B", &["A".into()]).unwrap();
        assert!(p.windows(2).all(|w| legal(&w[0], &w[1])));
        assert!(!p.contains(&"A".into()));
        assert!(shortest("NO", "B", &[]).is_none());
    }
    #[test]
    fn reverse_edges_and_shared_junctions_conflict() {
        let s = RoadState {
            leases: vec![RouteGrant {
                token: "g".into(),
                task_id: "t".into(),
                vehicle_id: "01".into(),
                stage: "pickup".into(),
                path: vec!["A".into(), "J2".into(), "B".into()],
                granted_at: 1,
                released_at: None,
            }],
        };
        assert!(!s
            .conflict(&["B".into(), "J2".into(), "A".into()], "02")
            .is_empty());
        assert!(!s.conflict(&["J2".into(), "J5".into()], "02").is_empty());
        assert!(s
            .conflict(&["C".into(), "J4".into(), "WIP".into()], "02")
            .is_empty());
    }
    #[test]
    fn motion_follows_edges_and_reaches_exact_endpoint() {
        let mut m = Motion::new(shortest("A", "B", &[]).unwrap()).unwrap();
        for _ in 0..40 {
            let (pos, edge, done) = m.step();
            assert_eq!(pos.1, 25.);
            if let Some((a, b)) = edge {
                assert!(legal(&a, &b));
            }
            if done {
                assert_eq!(pos, (80., 25.));
                return;
            }
        }
        panic!("route never completed");
    }
}
