pub mod projector;
pub mod schema;
pub mod store;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRow {
    pub seq: i64,
    pub run_id: String,
    pub ts: String,
    pub event_type: String,
    pub task_id: Option<String>,
    pub actor_role: Option<String>,
    pub actor_id: Option<String>,
    pub attempt: Option<i64>,
    pub payload_json: Value,
    pub dedupe_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub event_type: String,
    pub task_id: Option<String>,
    pub actor_role: Option<String>,
    pub actor_id: Option<String>,
    pub attempt: Option<i64>,
    pub payload_json: Value,
    pub dedupe_key: Option<String>,
}

impl NewEvent {
    pub fn simple(event_type: &str, payload_json: Value) -> Self {
        Self {
            event_type: event_type.to_string(),
            task_id: None,
            actor_role: None,
            actor_id: None,
            attempt: None,
            payload_json,
            dedupe_key: None,
        }
    }
}
