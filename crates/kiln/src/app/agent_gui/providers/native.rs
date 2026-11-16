use super::*;
use std::time::{Duration, Instant};
/// A cancelled write still owns its slot until ACK and readback settle. No later
/// write or Submit can race that reply. Cancellation never claims rollback.
#[derive(Default)]
pub(super) struct NativeFlight {
    pub operation: Option<NativeOperation>,
    pub reads: BTreeMap<String, Discovery>,
    read_started: BTreeMap<String, Instant>,
}
pub(super) struct NativeOperation {
    pub id: String,
    pub control: String,
    pub value: ControlValue,
    pub cancelled: bool,
    pub readback: Option<String>,
    started: Instant,
    timeout_reported: bool,
}
impl NativeOperation {
    pub fn outcome(
        &self,
        state: ApplicationState,
        effective: Option<ControlValue>,
        message: Option<String>,
    ) -> Event {
        Event::Controls(ControlEvent::Outcome(ControlOutcome {
            settled: false,
            operation: self.id.clone(),
            control: self.control.clone(),
            requested: self.value.clone(),
            effective,
            state,
            message,
        }))
    }
}
impl NativeFlight {
    pub fn idle(&self) -> Result<()> {
        ensure!(
            self.operation.is_none(),
            "A native configuration operation is still settling"
        );
        Ok(())
    }
    pub fn begin(&mut self, control: &str, value: ControlValue, e: &mut Effects) -> Result<String> {
        self.idle()?;
        let id = format!("native-{}", journal::unique());
        let op = NativeOperation {
            id: id.clone(),
            control: control.into(),
            value,
            cancelled: false,
            readback: None,
            started: Instant::now(),
            timeout_reported: false,
        };
        e.event(op.outcome(ApplicationState::Pending, None, None));
        self.operation = Some(op);
        Ok(id)
    }
    pub fn cancel(&mut self, e: &mut Effects) {
        if let Some(op) = &mut self.operation {
            op.cancelled = true;
            e.event(op.outcome(
                ApplicationState::Cancelled,
                None,
                Some(
                    "The sent setting may still take effect; waiting for native settlement.".into(),
                ),
            ));
        }
    }
    pub fn read(&mut self, target: Discovery) -> String {
        let id = format!("discovery-{}", journal::unique());
        self.reads.insert(id.clone(), target);
        self.read_started.insert(id.clone(), Instant::now());
        id
    }
    pub fn tick(&mut self) -> Effects {
        self.tick_at(Instant::now())
    }
    pub fn tick_at(&mut self, now: Instant) -> Effects {
        let mut e = Effects::default();
        if let Some(op) = &mut self.operation {
            if !op.timeout_reported
                && now.saturating_duration_since(op.started) >= Duration::from_secs(15)
            {
                op.timeout_reported = true;
                e.event(op.outcome(ApplicationState::Unconfirmed,None,Some("The native operation timed out without confirmation. Send remains blocked until native settlement; a new conversation can be opened explicitly.".into())));
            }
        }
        let expired: Vec<String> = self
            .read_started
            .iter()
            .filter(|(_, t)| now.saturating_duration_since(**t) >= Duration::from_secs(15))
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.read_started.remove(&id);
            if self
                .operation
                .as_ref()
                .is_some_and(|op| op.readback.as_ref() == Some(&id))
            {
                continue;
            }
            if let Some(target) = self.reads.remove(&id) {
                e.event(unavailable(&format!("{target:?} timed out")));
            }
        }
        self.read_started
            .retain(|id, _| self.reads.contains_key(id));
        e
    }
    pub fn finish(
        &mut self,
        state: ApplicationState,
        value: Option<ControlValue>,
        message: Option<String>,
        e: &mut Effects,
    ) {
        if let Some(op) = self.operation.take() {
            let mut event = op.outcome(
                if op.cancelled {
                    ApplicationState::Cancelled
                } else {
                    state
                },
                value,
                message,
            );
            if let Event::Controls(ControlEvent::Outcome(outcome)) = &mut event {
                outcome.settled = true;
            }
            e.event(event);
        }
    }
}
pub(super) fn status(subject: &str, raw: &Value, keys: &[&str]) -> Event {
    let fields = keys
        .iter()
        .filter_map(|key| {
            raw.get(*key)
                .and_then(ControlValue::from_safe)
                .map(|v| ((*key).into(), v))
        })
        .collect();
    Event::Controls(ControlEvent::Status(NativeStatus {
        subject: subject.into(),
        fields,
        unavailable: None,
    }))
}
pub(super) fn unavailable(subject: &str) -> Event {
    Event::Controls(ControlEvent::Status(NativeStatus {
        subject: subject.into(),
        fields: Default::default(),
        unavailable: Some("The native CLI did not provide this capability.".into()),
    }))
}
pub(super) fn acp_options(raw: &Value) -> Vec<ControlOption> {
    raw.as_array()
        .into_iter()
        .flatten()
        .take(256)
        .map(|o| {
            let mut choices = Vec::new();
            for group in o["options"].as_array().into_iter().flatten().take(512) {
                if let Some(items) = group["options"].as_array() {
                    let label = Some(controls::bounded(group, "name"));
                    for v in items.iter().take(512) {
                        choices.push(ControlChoice {
                            value: controls::bounded(v, "value"),
                            label: controls::bounded(v, "name"),
                            description: controls::bounded(v, "description"),
                            group: label.clone(),
                        });
                    }
                } else {
                    choices.push(ControlChoice {
                        value: controls::bounded(group, "value"),
                        label: controls::bounded(group, "name"),
                        description: controls::bounded(group, "description"),
                        group: None,
                    });
                }
            }
            let supported = o["type"] == "select";
            ControlOption {
                id: controls::bounded(o, "id"),
                label: controls::bounded(o, "name"),
                description: controls::bounded(o, "description"),
                category: o["category"].as_str().map(str::to_owned),
                kind: if supported {
                    ControlKind::Select
                } else {
                    ControlKind::Unsupported(controls::bounded(o, "type"))
                },
                choices,
                current: o.get("currentValue").and_then(ControlValue::from_safe),
                scope: ControlScope::Live,
                available: supported,
                reason: (!supported)
                    .then(|| "This option type is not supported by this client.".into()),
                source: Some("acp".into()),
                documentation: Some(
                    "https://agentclientprotocol.com/protocol/v1/session-config-options".into(),
                ),
            }
        })
        .collect()
}

/// Reports projection limits without copying any payload into the journal.
pub(super) fn coverage(
    subject: &str,
    raw: &Value,
    retained: usize,
    additional_partial: bool,
) -> Event {
    fn shortened(v: &Value) -> bool {
        match v {
            Value::String(v) => v.chars().count() > 2048,
            Value::Array(v) => v.iter().any(shortened),
            Value::Object(v) => v.iter().any(|(key, value)| {
                (matches!(key.as_str(), "argumentHint" | "hint")
                    && value.as_str().is_some_and(|s| s.chars().count() > 512))
                    || (key == "aliases" && value.as_array().is_some_and(|v| v.len() > 32))
                    || shortened(value)
            }),
            _ => false,
        }
    }
    let received = raw.as_array().map(Vec::len).unwrap_or(retained);
    let partial = additional_partial || received > retained || shortened(raw);
    Event::Controls(ControlEvent::Coverage(CatalogCoverage{subject:subject.into(),received,retained,partial,reason:partial.then(||"This bounded native snapshot is partial or contains shortened metadata (2048-character text, 512-character hint, and 32-alias limits). Refresh or use the native terminal for remaining entries.".into())}))
}
