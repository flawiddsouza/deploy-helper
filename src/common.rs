use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct Debug(pub IndexMap<String, String>);

#[derive(Debug, Deserialize, Serialize)]
pub struct Register {
    pub stdout: String,
    pub stderr: String,
    pub rc: i32,
}

#[derive(Debug, Deserialize)]
pub struct Task {
    pub name: String,
    pub shell: Option<String>,
    pub command: Option<String>,
    pub register: Option<String>,
    pub debug: Option<Debug>,
    pub vars: Option<IndexMap<String, String>>,
    pub chdir: Option<String>,
    pub when: Option<String>,
    pub r#loop: Option<Vec<Value>>,
    pub include_tasks: Option<String>,
}
