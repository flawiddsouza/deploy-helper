use colored::Colorize;
use indexmap::IndexMap;
use serde_json::Value;
use ssh2::Session;

use crate::common::{FileSpec, Register};
use crate::utils;

// `file:` manages a path's existence and attributes. Only `state: directory`
// is supported: create the directory (with parents), then apply mode/owner/
// group to the final component. Replaces `install -d -m ... -o ... -g ...`.
pub fn process(
    task_name: &str,
    spec: &FileSpec,
    is_localhost: bool,
    session: Option<&Session>,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    register: Option<&String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if spec.state != "directory" {
        return Err(format!(
            "Task '{}': file supports only state: directory (got '{}')",
            task_name, spec.state
        )
        .into());
    }

    let path = utils::replace_placeholders(&spec.path, vars_map);
    let mode = spec
        .mode
        .as_deref()
        .map(|m| utils::replace_placeholders(m, vars_map));
    if let Some(m) = &mode {
        utils::validate_mode(m).map_err(|e| format!("Task '{}': {}", task_name, e))?;
    }
    let owner = spec
        .owner
        .as_deref()
        .map(|o| utils::replace_placeholders(o, vars_map));
    let group = spec
        .group
        .as_deref()
        .map(|g| utils::replace_placeholders(g, vars_map));

    let escaped = utils::shell_escape(&path);
    let mut command = format!("mkdir -p {}", escaped);
    if let Some(m) = &mode {
        command.push_str(&format!(" && chmod {} {}", m, escaped));
    }
    match (&owner, &group) {
        (Some(o), Some(g)) => command.push_str(&format!(
            " && chown {}:{} {}",
            utils::shell_escape(o),
            utils::shell_escape(g),
            escaped
        )),
        (Some(o), None) => {
            command.push_str(&format!(" && chown {} {}", utils::shell_escape(o), escaped))
        }
        (None, Some(g)) => {
            command.push_str(&format!(" && chgrp {} {}", utils::shell_escape(g), escaped))
        }
        (None, None) => {}
    }

    let mut notes: Vec<String> = Vec::new();
    if let Some(m) = &mode {
        notes.push(format!("mode {}", m));
    }
    if let Some(o) = &owner {
        notes.push(format!("owner {}", o));
    }
    if let Some(g) = &group {
        notes.push(format!("group {}", g));
    }
    let note = if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join(", "))
    };
    println!("{}", format!("> [file] directory {}{}", path, note).magenta());

    let (out, stderr, code) = utils::run_shell_on_target(
        &command,
        is_localhost,
        session,
        become_enabled,
        become_method,
        become_password,
    )?;
    if code != 0 {
        // The doas-PTY path merges stderr into stdout, so fall back to it when stderr is empty.
        let detail = if stderr.trim().is_empty() {
            out.trim()
        } else {
            stderr.trim()
        };
        return Err(format!("Failed to create directory {}: {}", path, detail).into());
    }

    if let Some(reg) = register {
        let value = serde_json::to_value(Register {
            stdout: String::new(),
            stderr: String::new(),
            rc: 0,
        })?;
        vars_map.insert(reg.clone(), value);
        println!("{}", format!("Registering output to: {}", reg).yellow());
    }

    Ok(())
}
