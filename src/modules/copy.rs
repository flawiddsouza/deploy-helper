use colored::Colorize;
use indexmap::IndexMap;
use serde_json::Value;
use ssh2::Session;
use std::path::Path;

use crate::common::{CopySpec, Register};
use crate::utils;

pub fn process(
    task_name: &str,
    spec: &CopySpec,
    deploy_file_dir: &Path,
    is_localhost: bool,
    session: Option<&Session>,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    register: Option<&String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let dest = utils::replace_placeholders(&spec.dest, vars_map);

    let mode = spec
        .mode
        .as_deref()
        .map(|m| utils::replace_placeholders(m, vars_map));
    if let Some(m) = &mode {
        utils::validate_mode(m).map_err(|e| format!("Task '{}': {}", task_name, e))?;
    }

    // A directory src copies itself (recursively) here and yields None; file/content
    // srcs yield the bytes to write through the shared single-file path below.
    let bytes: Option<Vec<u8>> = match (&spec.src, &spec.content) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "Task '{}': copy requires exactly one of src or content (both were set)",
                task_name
            )
            .into());
        }
        (None, None) => {
            return Err(format!(
                "Task '{}': copy requires exactly one of src or content (neither was set)",
                task_name
            )
            .into());
        }
        (None, Some(content)) => Some(utils::replace_placeholders(content, vars_map).into_bytes()),
        (Some(src), None) => {
            let rendered_src = utils::replace_placeholders(src, vars_map);
            let resolved_src = utils::resolve_src_path(deploy_file_dir, &rendered_src);
            if resolved_src.is_dir() {
                if mode.is_some() {
                    return Err(format!(
                        "Task '{}': mode is not supported when src is a directory",
                        task_name
                    )
                    .into());
                }
                // Directory src: recursive overlay copy of its contents into dest.
                println!(
                    "{}",
                    format!(
                        "> [copy dir] {} -> {}",
                        resolved_src.to_string_lossy().replace('\\', "/"),
                        dest
                    )
                    .magenta()
                );
                utils::write_dir_to_target(
                    &resolved_src,
                    &dest,
                    is_localhost,
                    session,
                    become_enabled,
                    become_method,
                    become_password,
                )?;
                None
            } else {
                Some(std::fs::read(&resolved_src).map_err(|_| {
                    format!(
                        "Copy source not found: {}",
                        resolved_src.to_string_lossy().replace('\\', "/")
                    )
                })?)
            }
        }
    };

    if let Some(bytes) = bytes {
        let mode_note = mode
            .as_deref()
            .map(|m| format!(", mode {}", m))
            .unwrap_or_default();
        println!(
            "{}",
            format!("> [copy] {} ({} bytes{})", dest, bytes.len(), mode_note).magenta()
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
