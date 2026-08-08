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
    env: Option<&IndexMap<String, String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = if is_localhost {
        utils::execute_local_command(command, use_shell, display_output, chdir, login_shell, env)
    } else {
        // Remote exec always goes through the login shell, so the environment
        // travels as export lines ahead of the command. Braces keep the whole
        // thing one compound command, so a chdir's `cd dir && ...` guard still
        // covers the command and not just the first export.
        let remote_cmd = match env {
            Some(env) if !env.is_empty() => format!(
                "{{ {}; {}; }}",
                utils::env_export_lines(env).join("; "),
                command
            ),
            _ => command.to_string(),
        };
        utils::execute_ssh_command(
            session.unwrap(),
            &remote_cmd,
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

// Runs a `become_method: doas` command through a PTY so doas can read the
// password from its controlling terminal (doas opens /dev/tty directly and
// ignores piped stdin). The PTY merges stdout and stderr, so the combined
// stream is returned as stdout and registered output reflects that.
fn handle_doas_pty_execution(
    command: &str,
    is_localhost: bool,
    session: Option<&Session>,
    chdir: Option<&str>,
    register: Option<&String>,
    login_shell: bool,
    display_output: bool,
    password: &str,
    vars_map: &mut IndexMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let doas_cmd = utils::wrap_become_command(command, "doas", None);
    let result: Result<(String, String, i32), Box<dyn std::error::Error>> = if is_localhost {
        #[cfg(unix)]
        {
            utils::execute_local_doas_with_pty(&doas_cmd, password, display_output, chdir, login_shell)
        }
        #[cfg(not(unix))]
        {
            Err("doas with become_password is not supported on non-Unix platforms".into())
        }
    } else {
        utils::execute_ssh_doas_with_pty(
            session.unwrap(),
            &doas_cmd,
            password,
            display_output,
            chdir,
            login_shell,
        )
    };

    match result {
        Ok((stdout, stderr, exit_code)) => {
            if exit_code != 0 {
                return Err(format!(
                    "Command execution failed with exit status: {}. Stopping further tasks.",
                    exit_code
                )
                .red()
                .into());
            }
            if let Some(reg) = register {
                let val = serde_json::to_value(common::Register {
                    stdout,
                    stderr,
                    rc: exit_code,
                })?;
                vars_map.insert(reg.clone(), val);
                println!("{}", format!("Registering output to: {}", reg).yellow());
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
    shell_defaults: Option<&str>,
    environment: Option<&IndexMap<String, String>>,
    is_localhost: bool,
    session: Option<&Session>,
    task_chdir: Option<&str>,
    register: Option<&String>,
    login_shell: bool,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    no_log: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // shell_defaults (e.g. "set -euo pipefail") and environment: exports are
    // injected ahead of the block like the built-in set -e: they run but are
    // not echoed with the commands, so env values never reach the output.
    // Built before the echo so a bad environment fails without echoing.
    let defaults = shell_defaults
        .map(|d| utils::replace_placeholders(d, vars_map))
        .filter(|d| !d.trim().is_empty());
    let mut prelude = vec!["set -e".to_string()];
    if let Some(d) = defaults {
        prelude.push(d);
    }
    if let Some(env) = environment {
        let rendered = utils::render_env_values(env, vars_map)?;
        prelude.extend(utils::env_export_lines(&rendered));
    }

    if !no_log {
        for seg in &display_segments {
            let substituted = utils::replace_placeholders(seg, vars_map);
            println!("{}", format!("> {}", substituted).magenta());
        }
    }

    let substituted_source = utils::replace_placeholders(source, vars_map);
    let exec_source = format!("{}\n{}", prelude.join("\n"), substituted_source);

    let display_output = register.is_none() && !no_log;

    if become_enabled && become_method == "doas" && become_password.is_some() {
        return handle_doas_pty_execution(
            &exec_source,
            is_localhost,
            session,
            task_chdir,
            register,
            login_shell,
            display_output,
            become_password.unwrap(),
            vars_map,
        );
    }

    let exec_cmd = if become_enabled {
        utils::wrap_become_command(&exec_source, become_method, become_password)
    } else {
        exec_source
    };

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
        None,
    )
}

// Runs a `command:` task — each line is a standalone command exec'd directly
// (no shell interpretation, so no state sharing between lines).
pub fn process_command(
    commands: Vec<String>,
    environment: Option<&IndexMap<String, String>>,
    is_localhost: bool,
    session: Option<&Session>,
    task_chdir: Option<&str>,
    register: Option<&String>,
    login_shell: bool,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    no_log: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let env_rendered = match environment {
        Some(env) if !env.is_empty() => Some(utils::render_env_values(env, vars_map)?),
        _ => None,
    };

    for cmd in commands {
        let substituted_cmd = utils::replace_placeholders(&cmd, vars_map);
        if !no_log {
            println!("{}", format!("> {}", substituted_cmd).magenta());
        }

        let display_output = register.is_none() && !no_log;

        // become wraps the command in `sh -c`, and sudo/su/doas reset the
        // caller's environment, so the exports must ride inside the wrapper.
        let become_inner = match &env_rendered {
            Some(env) if become_enabled => format!(
                "{}; {}",
                utils::env_export_lines(env).join("; "),
                substituted_cmd
            ),
            _ => substituted_cmd.clone(),
        };

        if become_enabled && become_method == "doas" && become_password.is_some() {
            handle_doas_pty_execution(
                &become_inner,
                is_localhost,
                session,
                task_chdir,
                register,
                login_shell,
                display_output,
                become_password.unwrap(),
                vars_map,
            )?;
            continue;
        }

        let exec_cmd = if become_enabled {
            utils::wrap_become_command(&become_inner, become_method, become_password)
        } else {
            substituted_cmd
        };

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
            if become_enabled {
                None
            } else {
                env_rendered.as_ref()
            },
        )?;
    }

    Ok(())
}
