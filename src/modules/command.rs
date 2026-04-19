use colored::Colorize;
use indexmap::IndexMap;
use serde_json::Value;
use ssh2::Session;

use crate::common;
use crate::utils;

fn handle_command_execution(
    is_localhost: bool,
    session: Option<&Session>,
    command: &str,
    use_shell: bool,
    display_output: bool,
    chdir: Option<&str>,
    register: Option<&String>,
    login_shell: bool,
    vars_map: &mut IndexMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = if is_localhost {
        utils::execute_local_command(command, use_shell, display_output, chdir, login_shell)
    } else {
        utils::execute_ssh_command(
            session.unwrap(),
            command,
            use_shell,
            display_output,
            chdir,
            login_shell,
        )
    };

    match result {
        Ok((stdout, stderr, exit_status)) => {
            if exit_status != 0 {
                return Err(format!(
                    "Command execution failed with exit status: {}. Stopping further tasks.",
                    exit_status
                )
                .red()
                .into());
            }

            if let Some(register) = register {
                let register_value = serde_json::to_value(common::Register {
                    stdout: stdout.clone(),
                    stderr: stderr.clone(),
                    rc: exit_status,
                })?;
                vars_map.insert(register.clone(), register_value);
                println!(
                    "{}",
                    format!("Registering output to: {}", register).yellow()
                );
            }
        }
        Err(e) => {
            return Err(format!(
                "Command execution failed with error: {}. Stopping further tasks.",
                e
            )
            .red()
            .into());
        }
    }

    Ok(())
}

// Runs a multi-line `shell:` block as a single shell invocation so shell
// state (variables, cwd, traps, shell options) is shared across lines. The
// split segments are used only for display — each is echoed with `> ` before
// execution starts. `set -e` preserves the previous per-line stop-on-error
// behavior.
pub fn process_shell_block(
    source: &str,
    display_segments: Vec<String>,
    is_localhost: bool,
    session: Option<&Session>,
    task_chdir: Option<&str>,
    register: Option<&String>,
    login_shell: bool,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    for seg in &display_segments {
        let substituted = utils::replace_placeholders(seg, vars_map);
        println!("{}", format!("> {}", substituted).magenta());
    }

    let substituted_source = utils::replace_placeholders(source, vars_map);
    let exec_source = format!("set -e\n{}", substituted_source);

    let exec_cmd = if become_enabled {
        utils::wrap_become_command(&exec_source, become_method, become_password)
    } else {
        exec_source
    };

    let display_output = register.is_none();
    handle_command_execution(
        is_localhost,
        session,
        &exec_cmd,
        true,
        display_output,
        task_chdir,
        register,
        login_shell,
        vars_map,
    )
}

// Runs a `command:` task — each line is a standalone command exec'd directly
// (no shell interpretation, so no state sharing between lines).
pub fn process_command(
    commands: Vec<String>,
    is_localhost: bool,
    session: Option<&Session>,
    task_chdir: Option<&str>,
    register: Option<&String>,
    login_shell: bool,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    for cmd in commands {
        let substituted_cmd = utils::replace_placeholders(&cmd, vars_map);
        println!("{}", format!("> {}", substituted_cmd).magenta());

        let exec_cmd = if become_enabled {
            utils::wrap_become_command(&substituted_cmd, become_method, become_password)
        } else {
            substituted_cmd
        };

        let display_output = register.is_none();
        handle_command_execution(
            is_localhost,
            session,
            &exec_cmd,
            false,
            display_output,
            task_chdir,
            register,
            login_shell,
            vars_map,
        )?;
    }

    Ok(())
}
