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
    vars_map: &mut IndexMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = if is_localhost {
        utils::execute_local_command(command, use_shell, display_output, chdir)
    } else {
        utils::execute_ssh_command(session.unwrap(), command, use_shell, display_output, chdir)
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

pub fn process(
    commands: Vec<String>,
    is_localhost: bool,
    session: Option<&Session>,
    use_shell: bool,
    task_chdir: Option<&str>,
    register: Option<&String>,
    vars_map: &mut IndexMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    for cmd in commands {
        let substituted_cmd = utils::replace_placeholders(&cmd, vars_map);
        println!("{}", format!("> {}", substituted_cmd).magenta());

        let display_output = register.is_none();
        handle_command_execution(
            is_localhost,
            session,
            &substituted_cmd,
            use_shell,
            display_output,
            task_chdir,
            register,
            vars_map,
        )?;
    }

    Ok(())
}
