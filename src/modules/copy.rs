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
    let bytes: Vec<u8> = match (&spec.src, &spec.content) {
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
        (None, Some(content)) => utils::replace_placeholders(content, vars_map).into_bytes(),
        (Some(src), None) => {
            let rendered_src = utils::replace_placeholders(src, vars_map);
            let resolved_src = utils::resolve_src_path(deploy_file_dir, &rendered_src);
            std::fs::read(&resolved_src).map_err(|_| {
                format!(
                    "Copy source not found: {}",
                    resolved_src.to_string_lossy().replace('\\', "/")
                )
            })?
        }
    };

    let dest = utils::replace_placeholders(&spec.dest, vars_map);

    println!(
        "{}",
        format!("> [copy] {} ({} bytes)", dest, bytes.len()).magenta()
    );

    utils::write_to_target(
        &bytes,
        &dest,
        is_localhost,
        session,
        become_enabled,
        become_method,
        become_password,
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
