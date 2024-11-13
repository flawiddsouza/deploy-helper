mod utils;

use clap::{Arg, Command as ClapCommand};
use colored::*;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ssh2::Session;
use std::path::Path;
use std::process::exit;

#[derive(Debug, Deserialize)]
struct ServerConfig {
    hosts: IndexMap<String, TargetHost>,
}

#[derive(Debug, Deserialize)]
struct TargetHost {
    host: String,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    ssh_key_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Deployment {
    name: String,
    hosts: String,
    chdir: Option<String>,
    tasks: Vec<Task>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Task {
    name: String,
    shell: Option<String>,
    command: Option<String>,
    register: Option<String>,
    debug: Option<Debug>,
    vars: Option<IndexMap<String, String>>,
    chdir: Option<String>,
    when: Option<String>,
    r#loop: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
struct Debug(IndexMap<String, String>);

#[derive(Debug, Deserialize, Serialize)]
struct Register {
    stdout: String,
    stderr: String,
    rc: i32,
}

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
                let register_value = serde_json::to_value(Register {
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

fn process_commands(
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

fn should_run_task(condition: &Option<String>, vars_map: &IndexMap<String, Value>) -> bool {
    if let Some(cond) = condition {
        let template_str = format!("{{% if {} %}}true{{% else %}}false{{% endif %}}", cond);
        let rendered_cond = utils::replace_placeholders(&template_str, vars_map);
        if rendered_cond == "false" {
            false
        } else {
            true
        }
    } else {
        true
    }
}

fn process_debug(debug: &Debug, vars_map: &IndexMap<String, Value>) {
    println!("{}", "Debug:".blue());
    for (key, msg) in debug.0.iter() {
        println!("{}", format!("{}:", key).blue());
        let debug_msg = utils::replace_placeholders(msg, vars_map);
        println!("{}", format!("{}", debug_msg).blue());
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = ClapCommand::new("deploy-helper")
        .version("1.0.3")
        .about("Deployment helper tool")
        .arg(
            Arg::new("deploy_file")
                .help("The deployment YAML file")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("extra_vars")
                .short('e')
                .long("extra-vars")
                .value_name("VARS")
                .help("Set additional variables as key=value, JSON, or @file")
                .num_args(1),
        )
        .arg(
            Arg::new("server_file")
                .short('i')
                .long("inventory")
                .value_name("FILE")
                .help("The server configuration YAML file")
                .num_args(1),
        )
        .get_matches();

    let deploy_file = matches.get_one::<String>("deploy_file").unwrap();
    let extra_vars = matches.get_one::<String>("extra_vars").map(|s| s.as_str());
    let default_server_file = "servers.yml".to_string();
    let server_file = matches
        .get_one::<String>("server_file")
        .unwrap_or(&default_server_file);

    let server_config: ServerConfig = utils::read_yaml(server_file)?;
    let deployment_docs: Vec<Vec<Deployment>> = utils::read_yaml_multi(deploy_file)?;
    let deployments = deployment_docs.into_iter().flatten().collect::<Vec<_>>();

    let mut vars_map: IndexMap<String, Value> = IndexMap::new();

    if let Some(extra_vars) = extra_vars {
        if extra_vars.starts_with('@') {
            let extra_vars_file = &extra_vars[1..];
            let extra_vars_path = Path::new(extra_vars_file);
            if extra_vars_path.exists() {
                let yaml_vars: IndexMap<String, Value> = utils::read_yaml(extra_vars_file)?;
                vars_map.extend(yaml_vars);
            } else {
                eprintln!(
                    "{}",
                    format!("Extra vars file not found: {}", extra_vars_file).red()
                );
                exit(1);
            }
        } else if extra_vars.starts_with('{') {
            let json_vars: IndexMap<String, Value> = serde_json::from_str(extra_vars)?;
            vars_map.extend(json_vars);
        } else {
            for var in extra_vars.split(' ') {
                let parts: Vec<&str> = var.splitn(2, '=').collect();
                if parts.len() == 2 {
                    vars_map.insert(parts[0].to_string(), Value::String(parts[1].to_string()));
                }
            }
        }
    }

    for dep in deployments {
        println!("{}", format!("Starting deployment: {}\n", dep.name).green());

        let hosts: Vec<&str> = dep.hosts.split(',').map(|s| s.trim()).collect();

        let hosts_len = hosts.len();

        for host in hosts {
            if hosts_len > 1 {
                println!("{}", format!("Processing host: {}\n", host).blue());
            }

            if let Some(target_host) = server_config.hosts.get(host) {
                let is_localhost = target_host.host == "localhost";
                let session = if !is_localhost {
                    let port = target_host.port.unwrap_or(22); // Use default port 22 if not provided
                    let user = target_host
                        .user
                        .as_deref()
                        .ok_or("Missing user for remote host")?;
                    let password = target_host.password.as_deref();
                    let ssh_key_path = target_host.ssh_key_path.as_deref();

                    Some(utils::setup_ssh_session(
                        &target_host.host,
                        port,
                        user,
                        password,
                        ssh_key_path,
                    )?)
                } else {
                    None
                };

                for task in &dep.tasks {
                    if !should_run_task(&task.when, &vars_map) {
                        println!("{}", format!("Skipping task: {}\n", task.name).yellow());
                        continue;
                    }

                    println!("{}", format!("Executing task: {}", task.name).cyan());

                    let task_chdir = task.chdir.as_deref().or(dep.chdir.as_deref()); // Use task-level chdir if present, otherwise use top-level chdir

                    if let Some(vars) = &task.vars {
                        for (key, value) in vars {
                            let evaluated_value =
                                utils::replace_placeholders_vars(&value, &vars_map);
                            vars_map.insert(key.clone(), evaluated_value);
                        }
                    }

                    // Debug print to verify vars_map
                    // println!("Vars map: {:?}", vars_map);

                    let loop_items = task.r#loop.clone().unwrap_or_else(|| vec![Value::Null]);

                    for item in loop_items {
                        vars_map.shift_remove("item");

                        if !item.is_null() {
                            vars_map.insert("item".to_string(), item.clone());
                        }

                        if let Some(debug) = &task.debug {
                            process_debug(debug, &vars_map);
                        }

                        if let Some(shell_command) = &task.shell {
                            let commands = utils::split_commands(shell_command);
                            process_commands(
                                commands,
                                is_localhost,
                                session.as_ref(),
                                true,
                                task_chdir,
                                task.register.as_ref(),
                                &mut vars_map,
                            )?;
                        }

                        if let Some(command) = &task.command {
                            let commands = utils::split_commands(command);
                            process_commands(
                                commands,
                                is_localhost,
                                session.as_ref(),
                                false,
                                task_chdir,
                                task.register.as_ref(),
                                &mut vars_map,
                            )?;
                        }
                    }

                    println!();
                }
            } else {
                eprintln!(
                    "{}",
                    format!("No server config found for host: {}", host).red()
                );
            }
        }
    }

    Ok(())
}
