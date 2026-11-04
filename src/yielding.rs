//! Automatic relocation of an idle station occupant; no shared-road planning.
use super::*;

pub fn plan(s: &mut RcsState, parent: usize, target: &str, requester: &str) -> String {
    if s.tasks[parent].kind == "YIELD" {
        return "讓位車等待目標清空".into();
    }
    if !s.auto_dispatch {
        return "新任務派發已暫停，暫不建立讓位任務".into();
    }
    let parent_id = s.tasks[parent].id.clone();
    let cars: Vec<Value> = s
        .traffic
        .observations
        .iter()
        .filter(|v| traffic::location(v) == Some(target) && v["vehicle_id"] != requester)
        .cloned()
        .collect();
    let mut notes = vec![];
    for car in cars {
        let id = car["vehicle_id"].as_str().unwrap_or("");
        if let Some(child) = s.tasks.iter().rev().find(|t| {
            t.kind == "YIELD"
                && t.parent_task.as_deref() == Some(&parent_id)
                && t.requested_vehicle.as_deref() == Some(id)
                && t.status != "COMPLETED"
        }) {
            notes.push(format!(
                "讓位任務 {}：{} → {}（{}）",
                child.id, id, child.to, child.status
            ));
            continue;
        }
        if car["online"] != true
            || now().saturating_sub(car["observed_at"].as_u64().unwrap_or(0)) > 5000
        {
            notes.push(format!("{id} 未連線或車況過期，無法自動讓位"));
            continue;
        }
        if car["paused"] != false
            || !car["active"].is_null()
            || car["pending_callbacks"] != 0
            || s.job_locks.iter().any(|lock|lock.vehicle_id==id)
            || s.tasks
                .iter()
                .any(|t| !terminal(&t.status) && t.assigned_vehicle.as_deref() == Some(id))
            || s.roads.leases.iter().any(|l| l.vehicle_id == id)
            || s.traffic.leases.iter().any(|l| l.vehicle_id == id)
        {
            notes.push(format!("{id} 忙碌、暫停或回報／預留待確認，保持等待"));
            continue;
        }
        let mut parking: Vec<&str> = PARKING_NODES
            .into_iter()
            .filter(|p| {
                *p != target
                    && s.traffic.blockers(p, id).is_empty()
                    && !s
                        .tasks
                        .iter()
                        .any(|t| !terminal(&t.status) && (t.from == *p || t.to == *p))
            })
            .collect();
        parking.sort_by(|a, b| {
            let origin = node(target).unwrap();
            let d = |p: &str| {
                let xy = node(p).unwrap();
                (xy.0 - origin.0).powi(2) + (xy.1 - origin.1).powi(2)
            };
            d(a).total_cmp(&d(b)).then(a.cmp(b))
        });
        let Some(park) = parking.first() else {
            notes.push("沒有可用等待點，保持等待".into());
            continue;
        };
        if s.tasks.len() >= 10000 {
            notes.push("任務數量已達上限，保持等待".into());
            continue;
        }
        let ts = now();
        let child = Task {
            job_id: None,
            id: format!("YIELD-{ts}-{}", s.tasks.len() + 1),
            request_id: format!("yield-{parent_id}-{id}-{ts}"),
            kind: "YIELD".into(),
            parent_task: Some(parent_id.clone()),
            from: target.into(),
            to: (*park).into(),
            status: "QUEUED".into(),
            progress: 0,
            phase: "等待自動讓位派發".into(),
            created_at: ts,
            updated_at: ts,
            note: format!("為 {requester} 的任務 {parent_id} 清空 {target}"),
            last_seq: 0,
            assigned_vehicle: None,
            requested_vehicle: Some(id.into()),
            wait_reason: None,
            wait_since: None,
            timing: TaskTiming::default(),
            routes: vec![],
        };
        // Reserve the waiting point before dispatch so two relocations cannot select it.
        s.traffic.leases.push(traffic::Lease {
            node: (*park).into(),
            vehicle_id: id.into(),
            task_id: child.id.clone(),
            stage: "move".into(),
            at: ts,
        });
        s.tasks.push(child.clone());
        log_event(
            s,
            &child.id,
            &format!("自動讓位：{id} 從 {target} 移至 {park}，協助 {parent_id}"),
        );
        log_event(
            s,
            &parent_id,
            &format!("已建立讓位任務 {}：{id} → {park}", child.id),
        );
        notes.push(format!("已自動安排 {id} → {park}（{}）", child.id));
    }
    if notes.is_empty() {
        "等待其他車的點位預留解除".into()
    } else {
        notes.join("；")
    }
}

pub fn cancel_orphaned(s: &mut RcsState) {
    let orphaned: Vec<usize> = s
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            t.kind == "YIELD"
                && t.status == "QUEUED"
                && t.parent_task
                    .as_ref()
                    .is_some_and(|id| s.tasks.iter().any(|p| p.id == *id && terminal(&p.status)))
        })
        .map(|(i, _)| i)
        .collect();
    for i in orphaned {
        s.tasks[i].status = "CANCELED".into();
        s.tasks[i].phase = "原搬運任務已結束，取消尚未派發的讓位".into();
        s.tasks[i].updated_at = now();
        s.tasks[i].timing.canceled_at = Some(now());
        let id = s.tasks[i].id.clone();
        s.traffic.leases.retain(|l| l.task_id != id);
        log_event(s, &id, "原搬運任務已結束，取消排隊讓位");
        save(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> RcsState {
        let mut s = RcsState::default();
        s.tasks.push(serde_json::from_value(json!({"id":"T","request_id":"R","from":"A","to":"B","status":"RUNNING","progress":37,"phase":"取料","created_at":now(),"updated_at":now(),"note":"","last_seq":12,"assigned_vehicle":"CT-SIM-01"})).unwrap());
        s.traffic.refresh(&[
            json!({"vehicle_id":"CT-SIM-01","online":true,"x":15,"y":25,"paused":false,"active":{"task_id":"T"},"pending_callbacks":0}),
            json!({"vehicle_id":"CT-SIM-02","online":true,"x":80,"y":25,"paused":false,"active":null,"pending_callbacks":0}),
            json!({"vehicle_id":"CT-SIM-03","online":true,"x":65,"y":55,"paused":false,"active":null,"pending_callbacks":0})]);
        s
    }
    #[test]
    fn plans_one_real_move_and_reserves_nearest_free_waiting_point() {
        let mut s = state();
        plan(&mut s, 0, "B", "CT-SIM-01");
        assert_eq!(s.tasks.len(), 2);
        assert_eq!(s.tasks[1].kind, "YIELD");
        assert_eq!(s.tasks[1].parent_task.as_deref(), Some("T"));
        assert_eq!(s.tasks[1].from, "B");
        assert_eq!(s.tasks[1].to, "P2");
        assert_eq!(s.traffic.leases[0].task_id, s.tasks[1].id);
        plan(&mut s, 0, "B", "CT-SIM-01");
        assert_eq!(s.tasks.len(), 2);
    }
    #[test]
    fn does_not_move_busy_paused_offline_or_unconfirmed_cars() {
        for field in ["paused", "active", "online", "pending_callbacks"] {
            let mut s = state();
            s.traffic.observations[1][field] = match field {
                "paused" => json!(true),
                "active" => json!({"task_id":"OTHER"}),
                "online" => json!(false),
                _ => json!(1),
            };
            plan(&mut s, 0, "B", "CT-SIM-01");
            assert_eq!(s.tasks.len(), 1, "{field}");
        }
        let mut s = state();
        for p in ["P1", "P2", "I1", "I2"] {
            s.traffic.leases.push(traffic::Lease {
                node: p.into(),
                vehicle_id: "CT-SIM-01".into(),
                task_id: "OTHER".into(),
                stage: "destination".into(),
                at: now(),
            });
        }
        assert!(plan(&mut s, 0, "B", "CT-SIM-01").contains("沒有可用等待點"));
        assert_eq!(s.tasks.len(), 1);
    }
}
