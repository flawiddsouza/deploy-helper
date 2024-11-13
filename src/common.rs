use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Debug(pub IndexMap<String, String>);

#[derive(Debug, Deserialize, Serialize)]
pub struct Register {
    pub stdout: String,
    pub stderr: String,
    pub rc: i32,
}
