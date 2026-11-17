//! Provider-reported task activity. Snapshots replace background membership; bookends retain details.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub run: String,
    pub kind: String,
    pub description: String,
    pub status: String,
    pub summary: String,
    pub tool_id: String,
    pub output_file: String,
    pub background: bool,
    pub ambient: bool,
    pub duration_ms: Option<u64>,
}
impl Task {
    pub fn live(&self) -> bool {
        matches!(self.status.as_str(), "running" | "pending" | "paused")
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Snapshot(Vec<Task>),
    Update(Task),
    Output {task_id:String, run_id:String, text:String, truncated:bool, unavailable:bool},
}
#[derive(Clone, Debug, Default)]
pub struct State {
    pub tasks: BTreeMap<String, Task>,
    pub background: Option<BTreeSet<String>>,
    pub outputs: BTreeMap<(String,String),(String,bool,bool)>,
}
impl State {
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Output {task_id,run_id,text,truncated,unavailable} => { self.outputs.insert((task_id.clone(),run_id.clone()),(text.clone(),*truncated,*unavailable)); },
            Event::Snapshot(tasks) => {
                let ids: BTreeSet<_> = tasks.iter().map(|t| t.id.clone()).collect();
                if let Some(old) = &self.background {
                    for id in old.difference(&ids) {
                        if let Some(t) = self.tasks.get_mut(id) {
                            if t.live() {
                                t.status = "unconfirmed".into();
                            }
                        }
                    }
                }
                self.background = Some(ids);
                for t in tasks {
                    self.merge(t);
                    if let Some(current) = self.tasks.get_mut(&t.id) {
                        if current.run == t.run {
                            current.ambient = t.ambient;
                            current.background = true;
                        }
                    }
                }
            }
            Event::Update(task) => {
                self.merge(task);
            }
        }
    }
    fn merge(&mut self, patch: &Task) {
        if let Some(old) = self.tasks.get(&patch.id) {
            if !patch.run.is_empty() && !old.run.is_empty() && patch.run < old.run {
                return;
            }
        }
        let changed_run = self
            .tasks
            .get(&patch.id)
            .is_some_and(|old| !patch.run.is_empty() && patch.run != old.run);
        if changed_run {
            self.tasks.remove(&patch.id);
        }
        let old = self.tasks.entry(patch.id.clone()).or_default();
        if matches!(
            old.status.as_str(),
            "completed" | "failed" | "stopped" | "killed"
        ) && patch.live()
        {
            return;
        }
        macro_rules! copy { ($($field:ident),*) => { $(if !patch.$field.is_empty() {old.$field=patch.$field.clone();})* }; }
        copy!(
            id,
            run,
            kind,
            description,
            status,
            summary,
            tool_id,
            output_file
        );
        old.background |= patch.background;
        old.ambient |= patch.ambient;
        if patch.duration_ms.is_some() {
            old.duration_ms = patch.duration_ms;
        }
    }
    pub fn live(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values().filter(|t| {
            if t.background {
                self.background
                    .as_ref()
                    .map(|ids| ids.contains(&t.id))
                    .unwrap_or_else(|| t.live())
            } else {
                t.live()
            }
        })
    }
}
fn text(v: &Value, key: &str) -> String {
    v[key]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(8192)
        .collect()
}
pub fn parse(v: &Value) -> Option<Event> {
    let subtype = v["subtype"].as_str()?;
    if subtype == "background_tasks_changed" {
        let tasks = v["tasks"].as_array()?;
        return Some(Event::Snapshot(
            tasks
                .iter()
                .filter_map(|v| task(v, "running", true))
                .collect(),
        ));
    }
    if !matches!(
        subtype,
        "task_started" | "task_updated" | "task_progress" | "task_notification"
    ) {
        return None;
    }
    let status = match subtype {
        "task_started" | "task_progress" => "running",
        "task_notification" => v["status"].as_str().unwrap_or("unconfirmed"),
        _ => v["patch"]["status"].as_str().unwrap_or(""),
    };
    let mut t = task(v, status, v["is_backgrounded"].as_bool().unwrap_or(false))?;
    if subtype == "task_updated" {
        let patch = &v["patch"];
        t.description = text(patch, "description");
        t.background = patch["is_backgrounded"].as_bool().unwrap_or(false);
        t.summary = text(patch, "error");
    }
    Some(Event::Update(t))
}
fn task(v: &Value, status: &str, background: bool) -> Option<Task> {
    let id = text(v, "task_id");
    if id.is_empty() {
        return None;
    }
    Some(Task {
        id,
        run: text(v, "run_id"),
        kind: text(v, "task_type"),
        description: text(v, "description"),
        status: status.into(),
        summary: text(v, "summary"),
        tool_id: text(v, "tool_use_id"),
        output_file: text(v, "output_file"),
        background,
        ambient: v["ambient"].as_bool().unwrap_or(false)
            || v["skip_transcript"].as_bool().unwrap_or(false),
        duration_ms: v["usage"]["duration_ms"].as_u64(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_background_snapshot_replaces_membership_and_ignores_late_runs() {
        let mut s = State::default();
        for v in [
            json!({"subtype":"background_tasks_changed","tasks":[{"task_id":"shell","run_id":"02","task_type":"local_bash","description":"sleep 60"},{"task_id":"watch","task_type":"monitor","description":"watch logs","ambient":true}]}),
            json!({"subtype":"task_started","task_id":"shell","run_id":"02","description":"sleep 60","is_backgrounded":true}),
            json!({"subtype":"task_notification","task_id":"shell","run_id":"01","status":"completed"}),
        ] {
            s.apply(&parse(&v).unwrap());
        }
        assert_eq!(s.live().count(), 2);
        assert_eq!(s.live().filter(|t| !t.ambient).count(), 1);
        s.apply(&parse(&json!({"subtype":"task_progress","task_id":"shell","run_id":"02","description":"sleep 60","summary":"Waiting for output","usage":{"duration_ms":36000}})).unwrap());
        assert_eq!(s.tasks["shell"].duration_ms, Some(36000));
        s.apply(&parse(&json!({"subtype":"background_tasks_changed","tasks":[]})).unwrap());
        assert_eq!(s.live().count(), 0);
        s.apply(&parse(&json!({"subtype":"task_notification","task_id":"shell","run_id":"02","status":"completed","summary":"Done"})).unwrap());
        assert_eq!(s.tasks["shell"].status, "completed");
    }
}
#[cfg(test)]
mod replay_tests {
    use super::super::{Envelope, Event as JournalEvent, Transcript};
    use super::*;
    use serde_json::json;
    #[test]
    fn activity_is_durable_but_resets_for_a_new_cli_generation() {
        let mut t = Transcript::default();
        t.apply(&Envelope {
            sequence: 1,
            generation: "a".into(),
            event: JournalEvent::Generation,
        });
        let e=Envelope{sequence:2,generation:"a".into(),event:JournalEvent::TaskActivity(parse(&json!({"subtype":"task_started","task_id":"monitor","task_type":"monitor","description":"Watch tests","is_backgrounded":true})).unwrap())};
        let bytes = serde_json::to_vec(&e).unwrap();
        t.apply(&serde_json::from_slice(&bytes).unwrap());
        assert_eq!(t.activity.live().count(), 1);
        t.apply(&Envelope {
            sequence: 3,
            generation: "b".into(),
            event: JournalEvent::Generation,
        });
        assert_eq!(t.activity.live().count(), 0);
        t.apply(&e);
        assert_eq!(t.activity.live().count(), 0);
    }
}
