mod common;
mod modules;
mod utils;

use clap::{Arg, Command as ClapCommand};
use colored::Colorize;
use indexmap::IndexMap;
use serde::Deserialize;
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
    vars: Option<IndexMap<String, String>>,
    tasks: Vec<common::Task>,
}

fn process_tasks(
    tasks: &[common::Task],
    is_localhost: bool,
    session: Option<&Session>,
    dep_chdir: Option<&str>,
    vars_map: &mut IndexMap<String, Value>,
    deploy_file_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    for task in tasks {
        let task_name = utils::replace_placeholders(&task.name, vars_map);

        if !modules::when::process(&task.when, vars_map) {
            println!("{}", format!("Skipping task: {}\n", task_name).yellow());
            continue;
        }

        println!("{}", format!("Executing task: {}", task_name).cyan());

        if let Some(vars) = &task.vars {
            for (key, value) in vars {
                let evaluated_value = utils::replace_placeholders_vars(&value, vars_map);
                vars_map.insert(key.clone(), evaluated_value);
            }
        }

        // Use task-level chdir if present, otherwise use top-level chdir
        let task_chdir = task.chdir.as_deref().or(dep_chdir).map(|s| {
            utils::replace_placeholders(s, vars_map)
        });

        // Debug print to verify vars_map
        // println!("Vars map: {:?}", vars_map);

        let loop_items = task.r#loop.clone().unwrap_or_else(|| vec![Value::Null]);

        for item in loop_items {
            vars_map.shift_remove("item");

            if !item.is_null() {
                vars_map.insert("item".to_string(), item.clone());
            }

            if let Some(debug) = &task.debug {
                modules::debug::process(debug, vars_map);
            }

            if let Some(shell_command) = &task.shell {
                let commands = utils::split_commands(shell_command);
                modules::command::process(
                    commands,
                    is_localhost,
                    session,
                    true,
                    task_chdir.as_deref(),
                    task.register.as_ref(),
                    vars_map,
                )?;
            }

            if let Some(command) = &task.command {
                let commands = utils::split_commands(command);
                modules::command::process(
                    commands,
                    is_localhost,
                    session,
                    false,
                    task_chdir.as_deref(),
                    task.register.as_ref(),
                    vars_map,
                )?;
            }

            if let Some(include_file) = &task.include_tasks {
                println!(
                    "{}",
                    format!("Including tasks from: {}\n", include_file).blue()
                );
                let include_file_path = deploy_file_dir.join(include_file);
                let included_tasks =
                    modules::include_tasks::process(include_file_path.to_str().unwrap())?;
                process_tasks(
                    &included_tasks,
                    is_localhost,
                    session,
                    task_chdir.as_deref(),
                    vars_map,
                    deploy_file_dir,
                )?;
            }
        }

        println!();
    }

    Ok(())
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

    let deploy_file_path = Path::new(deploy_file);
    let deploy_file_dir = deploy_file_path.parent().unwrap_or(Path::new("."));

    for dep in deployments {
        println!("{}", format!("Starting deployment: {}\n", dep.name).green());

        let hosts: Vec<&str> = dep.hosts.split(',').map(|s| s.trim()).collect();

        let hosts_len = hosts.len();

        if let Some(dep_vars) = &dep.vars {
            for (key, value) in dep_vars {
                let evaluated_value = utils::replace_placeholders_vars(&value, &vars_map);
                vars_map.insert(key.clone(), evaluated_value);
            }
        }

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

                process_tasks(
                    &dep.tasks,
                    is_localhost,
                    session.as_ref(),
                    dep.chdir.as_deref(),
                    &mut vars_map,
                    deploy_file_dir,
                )?;
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
