use colored::Colorize;
use indexmap::IndexMap;
use serde_json::Value;
use ssh2::Session;
use std::collections::HashSet;

use crate::common::{Register, SystemdSpec, SystemdUnitSpec, SystemdUnitState};
use crate::utils;

fn state_command(state: &SystemdUnitState) -> &'static str {
    match state {
        SystemdUnitState::Started => "start",
        SystemdUnitState::Stopped => "stop",
        SystemdUnitState::Restarted => "restart",
        SystemdUnitState::Reloaded => "reload",
    }
}

fn append_checked_command(command: &mut String, shell_command: &str, error: &str) {
    command.push_str(&format!(
        "if ! {shell_command}; then\n  echo {} >&2\n  exit 1\nfi\n",
        utils::shell_escape(error)
    ));
}

fn append_enabled_state_check(command: &mut String, escaped_name: &str, expected_enabled: bool) {
    command.push_str(&format!(
        "unit_enabled_state_rc=0\nunit_enabled_state=$(systemctl is-enabled {escaped_name} 2>&1) || unit_enabled_state_rc=$?\n"
    ));
    if expected_enabled {
        command.push_str(&format!(
            "case \"${{unit_enabled_state_rc}}:${{unit_enabled_state}}\" in\n  0:enabled|0:enabled-runtime) ;;\n  *) printf 'systemd unit %s is not enabled (state: %s)\\n' {escaped_name} \"$unit_enabled_state\" >&2; exit 1 ;;\nesac\n"
        ));
    } else {
        command.push_str(&format!(
            "case \"$unit_enabled_state\" in\n  disabled|static|indirect|generated|transient|masked|masked-runtime|linked|linked-runtime) ;;\n  enabled|enabled-runtime|alias) printf 'systemd unit %s is still enabled (state: %s)\\n' {escaped_name} \"$unit_enabled_state\" >&2; exit 1 ;;\n  *) printf 'systemd failed to determine enabled state for unit %s (exit %s): %s\\n' {escaped_name} \"$unit_enabled_state_rc\" \"$unit_enabled_state\" >&2; exit 1 ;;\nesac\n"
        ));
    }
}

#[derive(Debug)]
struct SystemdUnitResolved {
    name: String,
    assert_result: Option<String>,
}

fn validate_units(
    task_name: &str,
    units: &[SystemdUnitSpec],
    vars_map: &IndexMap<String, Value>,
) -> Result<Vec<SystemdUnitResolved>, Box<dyn std::error::Error>> {
    if units.is_empty() {
        return Err(format!("Task '{}': systemd units must not be empty", task_name).into());
    }

    let mut units_resolved = Vec::with_capacity(units.len());
    let mut seen = HashSet::new();
    for unit in units {
        let name = utils::replace_placeholders(&unit.name, vars_map);
        if name.trim().is_empty() {
            return Err(
                format!("Task '{}': systemd unit name must not be empty", task_name).into(),
            );
        }
        if !seen.insert(name.clone()) {
            return Err(format!(
                "Task '{}': systemd unit '{}' is defined more than once",
                task_name, name
            )
            .into());
        }
        if unit.enabled.is_none()
            && unit.state.is_none()
            && !unit.assert_enabled
            && !unit.assert_active
            && unit.assert_result.is_none()
        {
            return Err(format!(
                "Task '{}': systemd unit '{}' has no requested operation or assertion",
                task_name, name
            )
            .into());
        }
        if unit.enabled == Some(false) && unit.assert_enabled {
            return Err(format!(
                "Task '{}': systemd unit '{}' cannot set enabled: false and assert_enabled: true",
                task_name, name
            )
            .into());
        }
        if matches!(unit.state.as_ref(), Some(SystemdUnitState::Stopped)) && unit.assert_active {
            return Err(format!(
                "Task '{}': systemd unit '{}' cannot set state: stopped and assert_active: true",
                task_name, name
            )
            .into());
        }
        let assert_result = unit
            .assert_result
            .as_ref()
            .map(|result| utils::replace_placeholders(result, vars_map));
        if assert_result
            .as_deref()
            .is_some_and(|result| result.trim().is_empty())
        {
            return Err(format!(
                "Task '{}': systemd unit '{}' assert_result must not be empty",
                task_name, name
            )
            .into());
        }
        units_resolved.push(SystemdUnitResolved {
            name,
            assert_result,
        });
    }
    Ok(units_resolved)
}

pub fn process(
    task_name: &str,
    spec: &SystemdSpec,
    is_localhost: bool,
    session: Option<&Session>,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    register: Option<&String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let units_resolved = validate_units(task_name, &spec.units, vars_map)?;
    let mut command = String::from("set -eu\n");

    if spec.daemon_reload {
        append_checked_command(
            &mut command,
            "systemctl daemon-reload",
            "systemd daemon-reload failed",
        );
    }

    for (unit, unit_resolved) in spec.units.iter().zip(&units_resolved) {
        let name = &unit_resolved.name;
        let escaped_name = utils::shell_escape(name);
        if let Some(enabled) = unit.enabled {
            let operation = if enabled { "enable" } else { "disable" };
            append_checked_command(
                &mut command,
                &format!("systemctl {} {}", operation, escaped_name),
                &format!("systemd failed to {} unit {}", operation, name),
            );
            if enabled {
                append_enabled_state_check(&mut command, &escaped_name, true);
            } else {
                append_enabled_state_check(&mut command, &escaped_name, false);
            }
        }
        if unit.assert_enabled && unit.enabled != Some(true) {
            append_enabled_state_check(&mut command, &escaped_name, true);
        }
        if let Some(state) = &unit.state {
            let operation = state_command(state);
            append_checked_command(
                &mut command,
                &format!("systemctl {} {}", operation, escaped_name),
                &format!("systemd failed to {} unit {}", operation, name),
            );
        }
        if unit.assert_active {
            append_checked_command(
                &mut command,
                &format!("systemctl is-active --quiet {}", escaped_name),
                &format!("systemd unit {} is not active", name),
            );
        }
        if let Some(result) = &unit_resolved.assert_result {
            let escaped_result = utils::shell_escape(result);
            command.push_str(&format!(
                "if ! actual_result=$(systemctl show -p Result --value {escaped_name}); then\n  echo {} >&2\n  exit 1\nfi\nif [ \"$actual_result\" != {escaped_result} ]; then\n  printf 'systemd unit %s result: expected %s, got %s\\n' {escaped_name} {escaped_result} \"$actual_result\" >&2\n  exit 1\nfi\n",
                utils::shell_escape(&format!("systemd failed to read result for unit {}", name)),
            ));
        }
    }

    println!(
        "{}",
        format!(
            "> [systemd] {} unit{}{}",
            units_resolved.len(),
            if units_resolved.len() == 1 { "" } else { "s" },
            if spec.daemon_reload {
                " (daemon reload)"
            } else {
                ""
            }
        )
        .magenta()
    );

    let (out, stderr, code) = utils::run_shell_on_target(
        &command,
        is_localhost,
        session,
        become_enabled,
        become_method,
        become_password,
    )?;
    if code != 0 {
        let detail = if stderr.trim().is_empty() {
            out.trim()
        } else {
            stderr.trim()
        };
        return Err(format!("Task '{}': systemd failed: {}", task_name, detail).into());
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

#[cfg(test)]
mod tests {
    use super::{state_command, validate_units};
    use crate::common::{SystemdUnitSpec, SystemdUnitState};
    use indexmap::IndexMap;

    #[test]
    fn systemd_state_commands_match_systemctl() {
        assert_eq!(state_command(&SystemdUnitState::Started), "start");
        assert_eq!(state_command(&SystemdUnitState::Stopped), "stop");
        assert_eq!(state_command(&SystemdUnitState::Restarted), "restart");
        assert_eq!(state_command(&SystemdUnitState::Reloaded), "reload");
    }

    #[test]
    fn systemd_rejects_duplicate_units() {
        let units = vec![
            SystemdUnitSpec {
                name: "app.service".to_string(),
                enabled: Some(true),
                state: None,
                assert_enabled: false,
                assert_active: false,
                assert_result: None,
            },
            SystemdUnitSpec {
                name: "app.service".to_string(),
                enabled: None,
                state: Some(SystemdUnitState::Started),
                assert_enabled: false,
                assert_active: false,
                assert_result: None,
            },
        ];
        let err = validate_units("Example", &units, &IndexMap::new()).unwrap_err();
        assert!(err.to_string().contains("defined more than once"));
    }

    #[test]
    fn systemd_rejects_units_without_work() {
        let units = vec![SystemdUnitSpec {
            name: "app.service".to_string(),
            enabled: None,
            state: None,
            assert_enabled: false,
            assert_active: false,
            assert_result: None,
        }];
        let err = validate_units("Example", &units, &IndexMap::new()).unwrap_err();
        assert!(err.to_string().contains("no requested operation"));
    }

    #[test]
    fn systemd_rejects_disable_with_enabled_assertion() {
        let units = vec![SystemdUnitSpec {
            name: "app.service".to_string(),
            enabled: Some(false),
            state: None,
            assert_enabled: true,
            assert_active: false,
            assert_result: None,
        }];
        let err = validate_units("Example", &units, &IndexMap::new()).unwrap_err();
        assert!(err.to_string().contains("cannot set enabled: false"));
    }

    #[test]
    fn systemd_rejects_stop_with_active_assertion() {
        let units = vec![SystemdUnitSpec {
            name: "app.service".to_string(),
            enabled: None,
            state: Some(SystemdUnitState::Stopped),
            assert_enabled: false,
            assert_active: true,
            assert_result: None,
        }];
        let err = validate_units("Example", &units, &IndexMap::new()).unwrap_err();
        assert!(err.to_string().contains("cannot set state: stopped"));
    }

    #[test]
    fn systemd_rejects_assert_result_resolved_to_empty() {
        let units = vec![SystemdUnitSpec {
            name: "app.service".to_string(),
            enabled: None,
            state: None,
            assert_enabled: false,
            assert_active: false,
            assert_result: Some("{{ expected_result }}".to_string()),
        }];
        let mut vars_map = IndexMap::new();
        vars_map.insert("expected_result".to_string(), "".into());
        let err = validate_units("Example", &units, &vars_map).unwrap_err();
        assert!(err.to_string().contains("assert_result must not be empty"));
    }
}
