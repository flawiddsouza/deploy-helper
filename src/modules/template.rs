use colored::Colorize;
use indexmap::IndexMap;
use serde_json::Value;
use ssh2::Session;
use std::fs;
use std::path::Path;

use crate::common::{Register, TemplateSpec};
use crate::utils;

pub fn process(
    task_name: &str,
    spec: &TemplateSpec,
    deploy_file_dir: &Path,
    is_localhost: bool,
    session: Option<&Session>,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    register: Option<&String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let src = utils::replace_placeholders(&spec.src, vars_map);
    let dest = utils::replace_placeholders(&spec.dest, vars_map);

    let resolved_src = utils::resolve_src_path(deploy_file_dir, &src);

    let raw = fs::read(&resolved_src).map_err(|_| {
        format!(
            "Template source not found: {}",
            resolved_src.to_string_lossy().replace('\\', "/")
        )
    })?;
    let text = std::str::from_utf8(&raw).map_err(|_| {
        format!(
            "Template source is not valid UTF-8: {}",
            resolved_src.display()
        )
    })?;

    let mode = spec
        .mode
        .as_deref()
        .map(|m| utils::replace_placeholders(m, vars_map));
    if let Some(m) = &mode {
        utils::validate_mode(m).map_err(|e| format!("Task '{}': {}", task_name, e))?;
    }

    let rendered = utils::replace_placeholders(text, vars_map);
    let bytes = rendered.into_bytes();

    let mode_note = mode
        .as_deref()
        .map(|m| format!(", mode {}", m))
        .unwrap_or_default();
    println!(
        "{}",
        format!("> [template] {} ({} bytes{})", dest, bytes.len(), mode_note).magenta()
    );

    utils::write_to_target(
        &bytes,
        &dest,
        is_localhost,
        session,
        become_enabled,
        become_method,
        become_password,
        mode.as_deref(),
    )?;

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
